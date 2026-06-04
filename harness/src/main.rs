mod repo;

use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::routing::get;
use axum::Router;
use futures::StreamExt;
use samod::{DocHandle, Repo};
use tokio::runtime::Runtime;

use shared::{AppStatus, WS_PORT};

/// Global flag set by the watcher thread when should_exit is detected.
/// The main thread polls this flag and performs cleanup.
static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);

fn main() {
    // ── 1. Create the tokio runtime and set up the shared doc ───────────
    let rt = Runtime::new().expect("create tokio runtime");

    let (repo, doc_handle) = rt.block_on(async { repo::start_repo().await });

    // Clear any stale pending app from a previous session.
    doc_handle.with_document(|doc| {
        use autosurgeon::{hydrate, reconcile};
        let mut agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
        agent.pending_app = None;
        agent.extension_requests = false;
        agent.should_exit = false;
        let mut tx = doc.transaction();
        reconcile(&mut tx, &agent).expect("reconcile");
        tx.commit();
    });

    let doc_handle_for_ws = doc_handle.clone();
    let doc_handle_for_watcher = doc_handle.clone();

    // ── 2. Start websocket server for pi extension on a tokio task ──────
    rt.spawn(async move {
        serve_ws(repo, doc_handle_for_ws).await;
    });

    // ── 3. Prepare temp files for IPC with makepad-host ─────────────────
    let runtime_dir = get_runtime_dir();

    // Ensure the runtime directory exists
    let _ = fs::create_dir_all(&runtime_dir);

    let splash_file = runtime_dir.join("splash_body.txt");
    let status_file = runtime_dir.join("host_status.txt");
    let window_marker = runtime_dir.join("window_ready.txt");

    // ── 4. Spawn the doc-change watcher on a separate thread ────────────
    let splash_file_for_watcher = splash_file.clone();
    let status_file_for_watcher = status_file.clone();

    // We need a second runtime because the watcher loop blocks on changes().
    std::thread::spawn(move || {
        let rt2 = Runtime::new().expect("create watcher runtime");
        rt2.block_on(async move {
            watch_doc_changes(
                doc_handle_for_watcher,
                splash_file_for_watcher,
                status_file_for_watcher,
            )
            .await;
        });
    });

    // ── 5. Spawn makepad-host as a separate process ────────────────────
    // Can be disabled via env var MAKEPAD_HOST_DISABLE=1 (useful for tests or headless)
    let host_disabled = env::var("MAKEPAD_HOST_DISABLE").ok().as_deref() == Some("1");

    let makepad_host_binary = if host_disabled {
        None
    } else {
        find_makepad_host_binary()
    };
    let mut host_process: Option<Child> = None;

    if let Some(binary) = makepad_host_binary {
        eprintln!("[harness] spawning makepad-host: {}", binary.display());

        match Command::new(&binary)
            .env("MAKEPAD_HOST_SPLASH_FILE", splash_file.to_str().unwrap())
            .env("MAKEPAD_HOST_STATUS_FILE", status_file.to_str().unwrap())
            .env("MAKEPAD_HOST_WINDOW_MARKER", window_marker.to_str().unwrap())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => {
                let pid = child.id();
                eprintln!("[harness] makepad-host spawned (PID: {})", pid);
                host_process = Some(child);
            }
            Err(err) => {
                eprintln!("[harness] failed to spawn makepad-host: {err}");
            }
        }
    } else {
        eprintln!("[harness] makepad-host binary not found — running headless");
    }

    // ── 6. Wait for shutdown signal ────────────────────────────────────
    // Block the main thread until SHOULD_EXIT flag is set (by the watcher
    // thread when it detects should_exit in the doc).
    while !SHOULD_EXIT.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(500));
    }

    eprintln!("[harness] shutting down");

    // ── 7. Cleanup ─────────────────────────────────────────────────────
    if let Some(mut child) = host_process {
        eprintln!("[harness] killing makepad-host (PID: {})", child.id());
        let _ = child.kill();
        let _ = child.wait();
    }

    // Clean up temp files
    let _ = fs::remove_file(&splash_file);
    let _ = fs::remove_file(&status_file);
    let _ = fs::remove_file(&window_marker);
    let _ = fs::remove_dir(&runtime_dir);

    eprintln!("[harness] goodbye");
}

// ── Temp directory helpers ───────────────────────────────────────────────

fn get_runtime_dir() -> PathBuf {
    let base = env::temp_dir();
    let pid = std::process::id();
    base.join(format!("a2app-harness-{}", pid))
}

fn find_makepad_host_binary() -> Option<PathBuf> {
    if let Ok(path) = env::var("MAKEPAD_HOST_BINARY") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Search relative to the harness binary
    if let Ok(exe_path) = env::current_exe() {
        // Look in the same directory
        let dir = exe_path.parent()?;

        // The makepad-host binary is typically in the same target dir
        // Try finding it in the workspace target directory
        let mut candidate_paths: Vec<PathBuf> = vec![
            dir.join("makepad-host"),
            dir.join("makepad-host.exe"),
        ];
        // Relative to project root
        if let Some(p) = dir.parent().and_then(|p| p.parent()).map(|p| p.join("makepad-host").join("target").join("debug").join("makepad-host")) {
            candidate_paths.push(p);
        }
        // In the workspace target directory
        if let Some(p) = dir.parent().and_then(|p| p.parent()).map(|p| p.join("target").join("debug").join("makepad-host")) {
            candidate_paths.push(p);
        }

        for candidate in &candidate_paths {
            if candidate.exists() {
                return Some(candidate.clone());
            }
        }
    }

    // Last resort: look in common locations
    let cwd_candidate = PathBuf::from("./target/debug/makepad-host");
    if cwd_candidate.exists() {
        return Some(cwd_candidate);
    }

    None
}

// ── Websocket server ─────────────────────────────────────────────────────

async fn serve_ws(repo: Repo, doc_handle: DocHandle) {
    kill_process_on_port(WS_PORT);

    let ws_app = Router::new()
        .route("/sync", get(ws_upgrade_handler))
        .with_state(repo);

    // Also serve a simple endpoint so the extension can discover the doc ID.
    let doc_id = doc_handle.document_id().to_string();
    let info_app = Router::new().route(
        "/doc_id",
        get(move || {
            let body = doc_id.clone();
            async move { body }
        }),
    );

    let combined = Router::new().merge(ws_app).merge(info_app);
    let addr = SocketAddr::from(([127, 0, 0, 1], WS_PORT));

    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            eprintln!("[harness] listening on 127.0.0.1:{WS_PORT}");
            if let Err(err) = axum::serve(listener, combined).await {
                eprintln!("[harness] server failed: {err}");
            }
        }
        Err(err) => {
            eprintln!("[harness] bind listener: {err} — port {WS_PORT} still in use");
        }
    }
}

async fn ws_upgrade_handler(
    State(repo): State<Repo>,
    ws: WebSocketUpgrade,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(err) = repo.accept_axum(socket) {
            eprintln!("[harness] failed to accept websocket peer: {err:?}");
        }
    })
}

fn kill_process_on_port(port: u16) {
    use std::process::Command;
    let output = Command::new("lsof")
        .args(["-ti", &format!(":{port}")])
        .output();
    if let Ok(output) = output {
        if !output.stdout.is_empty() {
            let pids = String::from_utf8_lossy(&output.stdout);
            for pid in pids.lines() {
                let pid = pid.trim();
                if !pid.is_empty() {
                    eprintln!("[harness] Killing stale process {pid} on port {port}");
                    let _ = Command::new("kill").args(["-9", pid]).status();
                }
            }
        }
    }
}

// ── Doc change watcher ───────────────────────────────────────────────────

async fn watch_doc_changes(
    doc_handle: samod::DocHandle,
    splash_file: PathBuf,
    status_file: PathBuf,
) {
    let mut changes = doc_handle.changes();

    eprintln!("[watcher] starting change listener");

    while changes.next().await.is_some() {
        eprintln!("[watcher] change detected!");

        // Read the current doc state
        let (needs_signal, should_exit, pending_app) = doc_handle.with_document(|doc| {
            use autosurgeon::hydrate;
            let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
            eprintln!(
                "[watcher] pending_app: {:?}, extension_requests: {}, should_exit: {}",
                agent.pending_app, agent.extension_requests, agent.should_exit
            );
            (
                agent.extension_requests,
                agent.should_exit,
                agent.pending_app.clone(),
            )
        });

        if should_exit {
            eprintln!("[watcher] should_exit received — signalling main thread");
            SHOULD_EXIT.store(true, Ordering::SeqCst);
            return;
        }

        if needs_signal {
            eprintln!("[watcher] extension_requests is true, processing");

            // Write the pending app data to the splash file for the makepad-host process
            if let Some(ref app) = pending_app {
                // The splash file format: first line = app_id, then blank line, then splash body
                let content = format!("{}\n{}", app.id, app.splash_body);
                let _ = fs::write(&splash_file, &content);
                eprintln!(
                    "[watcher] wrote splash file for app '{}' ({} bytes)",
                    app.id,
                    content.len()
                );

                // Update status: Pending → Launched
                if app.status == AppStatus::Pending {
                    doc_handle.with_document(|doc| {
                        use autosurgeon::{hydrate, reconcile};
                        let mut agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
                        if let Some(ref mut pa) = agent.pending_app {
                            pa.status = AppStatus::Launched;
                        }
                        let mut tx = doc.transaction();
                        reconcile(&mut tx, &agent).expect("reconcile");
                        tx.commit();
                    });
                    eprintln!("[watcher] app status updated to Launched");
                }
            } else {
                // No pending app — clear the splash file
                let _ = fs::write(&splash_file, "");
                eprintln!("[watcher] cleared splash file (no pending app)");
            }

            // Write ready status
            let _ = fs::write(&status_file, "ready");

            // Reset the flag
            doc_handle.with_document(|doc| {
                use autosurgeon::{hydrate, reconcile};
                let mut agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
                agent.extension_requests = false;
                let mut tx = doc.transaction();
                reconcile(&mut tx, &agent).expect("reconcile");
                tx.commit();
            });

            eprintln!("[watcher] extension_requests reset to false");
        }
    }
}
