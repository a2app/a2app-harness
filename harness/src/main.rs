use std::env;
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use samod::DocHandle;
use serde::{Deserialize, Serialize};
use shared::AgentDoc;
use tokio::runtime::Runtime;

// ── Ports ────────────────────────────────────────────────────────────────

/// JSON WebSocket — pi extension ↔ harness (simple JSON messages, no CRDT)
const JSON_WS_PORT: u16 = 2341;

/// samod WebSocket — harness ↔ makepad-host (CRDT sync between two Rust processes)
const SAMOD_WS_PORT: u16 = 2342;

// ── JSON WS message types (pi ↔ harness) ─────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum PiToHarnessMsg {
    #[serde(rename = "launch")]
    Launch { app_id: String, splash_body: String },
    #[serde(rename = "clear")]
    Clear { app_id: String },
    #[serde(rename = "debug")]
    Debug { app_id: String, command: String, params: String },
    #[serde(rename = "send_pi_response")]
    SendPiResponse { app_id: String, data: String },
    #[serde(rename = "send_streaming_delta")]
    SendStreamingDelta { app_id: String, delta: String },
    #[serde(rename = "send_streaming_end")]
    SendStreamingEnd { app_id: String, final_text: String },
    #[serde(rename = "get_doc")]
    GetDoc,
    #[serde(rename = "exit")]
    Exit,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum HarnessToPiMsg {
    #[serde(rename = "welcome")]
    Welcome,
    #[serde(rename = "status")]
    Status { app_id: String, status: String },
    #[serde(rename = "user_response")]
    UserResponse { app_id: String, response: String },
    #[serde(rename = "error")]
    Error { app_id: String, message: String },
    #[serde(rename = "debug_response")]
    DebugResponse { app_id: String, result: String },
    #[serde(rename = "doc_state")]
    DocState { app_id: Option<String>, user_response: Option<String>, error_message: Option<String>, status: Option<String>, pi_response: Option<String> },
}

// ── Startup ──────────────────────────────────────────────────────────────

fn main() {
    // Only enable tracing if RUST_LOG is set
    let _ = tracing_subscriber::fmt::try_init();

    // If headless mode, skip spawning makepad-host (for testing)
    let headless = env::var("HARNESS_HEADLESS").ok().as_deref() == Some("1");

    // Start the background async runtime on a separate thread
    std::thread::spawn(move || {
        let rt = Runtime::new().expect("create tokio runtime");
        rt.block_on(background_main(headless));
        eprintln!("[harness] background tasks finished");
    });

    // Main thread: just wait until background signals exit, or block forever
    // In the future, this could do other things like run a CLI
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

// ── Background: bridge logic ─────────────────────────────────────────────

async fn background_main(headless: bool) {
    // ── 1. Create samod repo and shared doc ──────────────────────────
    // IMPORTANT: samod uses InMemoryStorage by default (no disk persistence).
    // The `load()` call just initializes the runtime, it does NOT load from disk.
    // Every harness restart starts with a fresh, empty CRDT document.
    // See samod docs: Repo::build_tokio() returns RepoBuilder<InMemoryStorage, ...>
    let repo = samod::Repo::build_tokio().load().await;

    // Create a fresh in-memory automerge document with default AgentDoc
    let mut initial = automerge::Automerge::new();
    {
        let mut tx = initial.transaction();
        autosurgeon::reconcile(&mut tx, &AgentDoc::default())
            .expect("reconcile default agent doc");
        tx.commit();
    }

    let doc_handle = repo
        .create(initial)
        .await
        .expect("create shared document");

    // Safety: clear all fields in case the default had any non-null values.
    // Even though we create a fresh doc, this ensures determinism.
    doc_handle.with_document(|doc| {
        use autosurgeon::{hydrate, reconcile};
        let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
        agent.pending_app = None;
        agent.extension_requests = false;
        agent.should_exit = false;
        agent.user_response = None;
        agent.error_message = None;
        agent.debug_command = None;
        agent.debug_response = None;
        let mut tx = doc.transaction();
        reconcile(&mut tx, &agent).expect("reconcile");
        tx.commit();
    });

    let doc_id = doc_handle.document_id().to_string();
    eprintln!("[harness] shared doc ID: {doc_id}");

    // ── 2. Set up samod WS server for makepad-host ─────────────────
    // In samod 0.6.1, accept_axum is called directly on the Repo.
    let repo_clone = repo.clone();
    tokio::spawn(async move {
        let ws_app = Router::new()
            .route("/sync", get(move |ws: WebSocketUpgrade| {
                let repo = repo_clone.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        if let Err(e) = repo.accept_axum(socket) {
                            eprintln!("[harness] accept makepad-host WS: {e:?}");
                        }
                    })
                }
            }));
        let addr = SocketAddr::from(([127, 0, 0, 1], SAMOD_WS_PORT));
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                eprintln!("[harness] samod WS listening on 127.0.0.1:{SAMOD_WS_PORT}");
                let _ = axum::serve(listener, ws_app).await;
            }
            Err(e) => eprintln!("[harness] samod WS bind: {e}"),
        }
    });

    // ── 3. Spawn makepad-host (unless headless) ──────────────────────
    let mut makepad_child: Option<Child> = None;
    if !headless {
        let ready_marker = format!("/tmp/makepad-host-ready-{}.marker", std::process::id());
        let _ = std::fs::remove_file(&ready_marker);

        let harness_bin = env::current_exe().ok();
        let makepad_bin = if let Some(ref bin) = harness_bin {
            // Same directory as the harness binary
            let mut p = bin.parent().unwrap().to_path_buf();
            p.push("makepad-host");
            if p.exists() { p } else {
                // Fallback: try sibling directory
                let mut p = bin.parent().unwrap().parent().unwrap().to_path_buf();
                p.push("makepad-host");
                p.push("target");
                p.push("debug");
                p.push("makepad-host");
                p
            }
        } else {
            // Guess: sibling in workspace
            let mut p = env::current_dir().unwrap_or_default();
            p.push("target");
            p.push("debug");
            p.push("makepad-host");
            p
        };

        eprintln!("[harness] spawning makepad-host: {}", makepad_bin.display());

        match Command::new(&makepad_bin)
            .env("MAKEPAD_HOST_DOC_ID", &doc_id)
            .env("MAKEPAD_HOST_WS_URL", format!("ws://127.0.0.1:{SAMOD_WS_PORT}/sync"))
            .env("MAKEPAD_HOST_READY_MARKER", &ready_marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => {
                makepad_child = Some(child);
                eprintln!("[harness] makepad-host spawned, waiting for ready marker...");

                // Wait for makepad-host to signal readiness (with timeout)
                let deadline = std::time::Instant::now() + Duration::from_secs(30);
                let mut ready = false;
                while std::time::Instant::now() < deadline {
                    if std::fs::read_to_string(&ready_marker)
                        .ok()
                        .map(|s| s.trim() == "ready")
                        .unwrap_or(false)
                    {
                        ready = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                if ready {
                    eprintln!("[harness] makepad-host is ready");
                } else {
                    eprintln!("[harness] WARNING: makepad-host did not become ready within timeout");
                }
                let _ = std::fs::remove_file(&ready_marker);
            }
            Err(e) => {
                eprintln!("[harness] failed to spawn makepad-host: {e}");
                eprintln!("[harness] continuing without makepad-host (headless-like mode)");
            }
        }
    }

    // ── 4. Set up JSON WS server for pi extension ────────────────────
    // Wrap doc_handle in a shared state for the WS handler
    let pending_interaction = Arc::new(AtomicBool::new(false));
    let bridge_state = BridgeState {
        doc: doc_handle.clone(),
        pi_tx: tokio::sync::broadcast::channel(16).0,
        pending_interaction: pending_interaction.clone(),
    };
    let bridge = std::sync::Arc::new(tokio::sync::Mutex::new(bridge_state));

    let bridge_for_ws = bridge.clone();
    let doc_id_for_http = doc_id.clone();
    tokio::spawn(async move {
        // Serve both the JSON WS endpoint and a /doc_id HTTP endpoint
        let ws_routes = Router::new()
            .route("/", get(move |ws: WebSocketUpgrade| {
                let bridge = bridge_for_ws.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        handle_pi_ws(socket, bridge).await;
                    })
                }
            }));
        let info_routes = Router::new().route(
            "/doc_id",
            get({
                let doc_id = doc_id_for_http.clone();
                move || {
                    let body = doc_id.clone();
                    async move { body }
                }
            }),
        );
        let json_app = Router::new().merge(ws_routes).merge(info_routes);
        let addr = SocketAddr::from(([127, 0, 0, 1], JSON_WS_PORT));
        // Kill stale processes on port
        kill_process_on_port(JSON_WS_PORT);
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                eprintln!("[harness] JSON WS listening on 127.0.0.1:{JSON_WS_PORT}");
                let _ = axum::serve(listener, json_app).await;
            }
            Err(e) => eprintln!("[harness] JSON WS bind: {e}"),
        }
    });

    // ── 5. Bridge loop: doc changes → push to pi ────────────────────
    // We also need to handle writes FROM pi in the WS handler above.
    // Here we watch for doc changes (from makepad-host) and push to pi.
    let mut doc_changes = doc_handle.changes();
    // Track last sent values to avoid forwarding duplicates.
    // user_response_version is monotonically incremented by makepad-host
    // on each write, so same-value responses are distinguishable.
    let mut last_user_response_version: u64 = 0;
    let mut last_error_message: Option<String> = None;
    let mut last_status: Option<String> = None;
    let mut last_status_app_id: Option<String> = None;

    while let Some(_change) = doc_changes.next().await {
        // If pi sent an interactive command (click/type_text), clear tracking
        // so the resulting user_response is forwarded even if value hasn't changed.
        // IMPORTANT: skip this doc iteration entirely — the current doc still has
        // the stale user_response from before the interaction. The actual response
        // comes on the NEXT doc change after the splash processes the event.
        if pending_interaction.swap(false, Ordering::SeqCst) {
            // Skip this doc iteration — the current state is from the debug_command write,
            // not yet from the splash processing the interaction.
            continue;
        }

        let (has_response, version, app_id, status, error_message, debug_response, _streaming_text, exit) = doc_handle.with_document(|doc| {
            use autosurgeon::hydrate;
            let agent: AgentDoc = hydrate(doc).unwrap_or_default();
            let app_id = agent.pending_app.as_ref().map(|a| a.id.clone());
            let status = agent.pending_app.as_ref().map(|a| match &a.status {
                shared::AppStatus::Pending => "Pending".to_string(),
                shared::AppStatus::Launched => "Launched".to_string(),
            });
            (agent.user_response.clone(), agent.user_response_version, app_id, status, agent.error_message.clone(), agent.debug_response.clone(), agent.streaming_text, agent.should_exit)
        });

        // Push user_response to pi if version changed
        if version != 0 && version != last_user_response_version {
            if let Some(ref response) = has_response {
                if let Some(ref id) = app_id {
                    let msg = HarnessToPiMsg::UserResponse {
                        app_id: id.clone(),
                        response: response.clone(),
                    };
                    let json = serde_json::to_string(&msg).unwrap_or_default();
                    let _ = bridge.lock().await.pi_tx.send(json);
                }
            }
            last_user_response_version = version;
        }

        // Push debug_response to pi if present
        if let Some(ref result) = debug_response {
            if let Some(ref id) = app_id {
                let msg = HarnessToPiMsg::DebugResponse {
                    app_id: id.clone(),
                    result: result.clone(),
                };
                let json = serde_json::to_string(&msg).unwrap_or_default();
                let _ = bridge.lock().await.pi_tx.send(json);
                // Clear the debug_response after forwarding
                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    agent.debug_response = None;
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                });
            }
        }

        // Push status update to pi only if changed
        if status != last_status || app_id != last_status_app_id {
            if let Some(ref id) = app_id {
                if let Some(ref st) = status {
                    let msg = HarnessToPiMsg::Status {
                        app_id: id.clone(),
                        status: st.clone(),
                    };
                    let json = serde_json::to_string(&msg).unwrap_or_default();
                    let _ = bridge.lock().await.pi_tx.send(json);
                }
            }
            last_status = status;
            last_status_app_id = app_id.clone();
        }

        // Push error message to pi only if changed
        if error_message != last_error_message {
            if let Some(ref msg_text) = error_message {
                if let Some(ref id) = app_id {
                    let msg = HarnessToPiMsg::Error {
                        app_id: id.clone(),
                        message: msg_text.clone(),
                    };
                    let json = serde_json::to_string(&msg).unwrap_or_default();
                    let _ = bridge.lock().await.pi_tx.send(json);
                }
            }
            last_error_message = error_message;
        }

        if exit {
            eprintln!("[harness] should_exit — stopping");
            break;
        }
    }

    // ── Cleanup ─────────────────────────────────────────────────────
    if let Some(mut child) = makepad_child {
        let _ = child.kill();
        let _ = child.wait();
    }

    eprintln!("[harness] bridge loop ended");
}

// ── Bridge state ─────────────────────────────────────────────────────────

struct BridgeState {
    doc: DocHandle,
    /// Broadcast channel for pushing messages from the bridge loop
    /// to the connected pi WebSocket.
    pi_tx: tokio::sync::broadcast::Sender<String>,
    /// Set to true when pi sends an interactive debug command (click/type_text);
    /// bridge loop clears user_response tracking on next iteration so the
    /// resulting user_response is forwarded even if the value hasn't changed.
    pending_interaction: Arc<AtomicBool>,
}

// ── Handle pi WebSocket connection ───────────────────────────────────────

async fn handle_pi_ws(ws: WebSocket, bridge: std::sync::Arc<tokio::sync::Mutex<BridgeState>>) {
    let (mut ws_tx, mut ws_rx) = ws.split();

    use tokio::sync::mpsc;
    let (fwd_tx, mut fwd_rx) = mpsc::unbounded_channel::<String>();

    // Spawn a task to forward messages → pi WS
    let fwd_handle = tokio::spawn(async move {
        while let Some(msg) = fwd_rx.recv().await {
            if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Helper to send a message to pi
    let send_to_pi = |msg: HarnessToPiMsg| {
        let json = serde_json::to_string(&msg).unwrap_or_default();
        let _ = fwd_tx.send(json);
    };

    // Send welcome
    send_to_pi(HarnessToPiMsg::Welcome);
    eprintln!("[harness] pi connected");

    // Subscribe to the broadcast channel for doc changes
    let mut pi_rx = bridge.lock().await.pi_tx.subscribe();

    // Spawn a task to forward broadcast messages → fwd_tx
    let fwd_tx2 = fwd_tx.clone();
    tokio::spawn(async move {
        while let Ok(msg) = pi_rx.recv().await {
            let _ = fwd_tx2.send(msg);
        }
    });

    // Read messages from pi
    let doc_handle = bridge.lock().await.doc.clone();
    while let Some(Ok(msg)) = ws_rx.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Binary(_) => continue,
            Message::Close(_) | Message::Ping(_) | Message::Pong(_) => continue,
        };

        let parsed: PiToHarnessMsg = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[harness] bad JSON from pi: {e}");
                continue;
            }
        };

        match parsed {
            PiToHarnessMsg::Launch { app_id, splash_body } => {
                eprintln!("[harness] pi: launch app '{app_id}' ({} chars)", splash_body.len());

                // First write: set Pending status
                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    // Clear stale state
                    agent.user_response = None;
                    agent.error_message = None;
                    agent.pending_app = Some(shared::PendingApp {
                        id: app_id.clone(),
                        splash_body: splash_body.clone(),
                        status: shared::AppStatus::Pending,
                    });
                    agent.extension_requests = true;
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                });

                // Second write: immediately advance to Launched so the pi extension
                // receives "Launched" status (not "Pending") and starts its debounce
                // window during which errors from makepad-host can arrive.
                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    if let Some(ref mut app) = agent.pending_app {
                        app.status = shared::AppStatus::Launched;
                    }
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                });
            }
            PiToHarnessMsg::Debug { app_id, command, params } => {
                eprintln!("[harness] pi: debug '{command}' on app '{app_id}'");

                // For interactive commands, signal the bridge loop to clear
                // user_response tracking so the response is forwarded even if
                // its value hasn't changed (e.g. toggle stays "true").
                if command == "click" || command == "type_text" {
                    bridge.lock().await.pending_interaction.store(true, Ordering::SeqCst);
                }

                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    // Clear any stale debug response before setting new command
                    agent.debug_response = None;
                    agent.debug_command = Some(shared::DebugCommand {
                        command: command.clone(),
                        params: params.clone(),
                    });
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                });
            }
            PiToHarnessMsg::GetDoc => {
                eprintln!("[harness] pi: get_doc");

                doc_handle.with_document(|doc| {
                    use autosurgeon::hydrate;
                    let agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    let app_id = agent.pending_app.as_ref().map(|a| a.id.clone());
                    let status = agent.pending_app.as_ref().map(|a| match &a.status {
                        shared::AppStatus::Pending => "Pending".to_string(),
                        shared::AppStatus::Launched => "Launched".to_string(),
                    });
                    let msg = HarnessToPiMsg::DocState {
                        app_id,
                        user_response: agent.user_response,
                        error_message: agent.error_message,
                        status,
                        pi_response: agent.pi_response,
                    };
                    let json = serde_json::to_string(&msg).unwrap_or_default();
                    let _ = fwd_tx.send(json);
                });
            }
            PiToHarnessMsg::SendStreamingDelta { app_id, delta } => {
                eprintln!("[harness] pi: streaming delta to app '{app_id}': {} chars", delta.len());

                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    // The extension sends the full accumulated text each time
                    // (via a 100ms timer), so we SET rather than APPEND.
                    agent.streaming_text = Some(delta);
                    agent.extension_requests = true;
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                });
            }

            PiToHarnessMsg::SendStreamingEnd { app_id, final_text } => {
                eprintln!("[harness] pi: streaming end to app '{app_id}': {} chars", final_text.len());

                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    // Move final text to pi_response and clear streaming
                    agent.pi_response = Some(final_text.clone());
                    agent.streaming_text = None;
                    agent.extension_requests = true;
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                });
            }

            PiToHarnessMsg::SendPiResponse { app_id, data } => {
                eprintln!("[harness] pi: send_pi_response to app '{app_id}': {} chars", data.len());

                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    agent.pi_response = Some(data.clone());
                    // Clear streaming state if a pi_response arrives (it supersedes streaming)
                    agent.streaming_text = None;
                    agent.extension_requests = true;
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                });
            }
            PiToHarnessMsg::Clear { app_id } => {
                eprintln!("[harness] pi: clear app '{app_id}'");

                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    agent.pending_app = None;
                    agent.extension_requests = true;
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                });
            }
            PiToHarnessMsg::Exit => {
                eprintln!("[harness] pi: exit");

                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    agent.should_exit = true;
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                });

                break;
            }
        }
    }

    eprintln!("[harness] pi disconnected");
    fwd_handle.abort();
}

// ── Helpers ──────────────────────────────────────────────────────────────

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
