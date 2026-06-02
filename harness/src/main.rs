mod doc;
mod repo;
mod router;

use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Json, State};
use axum::response::IntoResponse;
use axum::routing::post;
use serde::Deserialize;
use axum::routing::get;
use axum::Router;
use samod::Repo;
use shared::{StoredValue, DOC_ID_PORT, WS_PORT};

#[tokio::main]
async fn main() {
    let (repo_handle, doc_handle) = repo::start_repo().await;

    // Clear any stale mini_apps from a previous session so old apps don't show up.
    doc_handle.with_doc_mut(|agent| {
        agent.mini_apps.clear();
        agent.should_exit = false;
    });

    let doc_id = doc_handle.document_id().to_string();
    tokio::spawn(serve_harness_api(repo_handle.repo.clone(), doc_handle.clone(), doc_id));

    let mut makepad_child = spawn_makepad_host();

    router::run(doc_handle).await;

    // After router::run returns (should_exit was set), wait for makepad-host to exit.
    if let Some(ref mut child) = makepad_child {
        let _ = child.wait();
    }
}

/// Helper to kill any process on a given port using lsof.
fn kill_process_on_port(port: u16) {
    use std::process::Command as StdCommand;
    let output = StdCommand::new("lsof")
        .args(["-ti", &format!(":{port}")])
        .output();
    if let Ok(output) = output {
        if !output.stdout.is_empty() {
            let pids = String::from_utf8_lossy(&output.stdout);
            for pid in pids.lines() {
                let pid = pid.trim();
                if !pid.is_empty() {
                    eprintln!("[Harness] Killing stale process {pid} on port {port}");
                    let _ = StdCommand::new("kill").args(["-9", pid]).status();
                }
            }
        }
    }
}

use crate::repo::DocHandle;

#[derive(Clone)]
struct ApiState {
    repo: Repo,
    doc_handle: DocHandle,
}

#[derive(Deserialize)]
struct InferenceResponsePayload {
    app_id: String,
    content: String,
}

async fn serve_harness_api(repo: Repo, doc_handle: DocHandle, doc_id: String) {
    let api_state = ApiState {
        repo: repo.clone(),
        doc_handle: doc_handle.clone(),
    };

    let doc_id_app = Router::new()
        .route("/doc_id",
        get({
            let doc_id = doc_id.clone();
            move || {
                let body = doc_id.clone();
                async move { body }
            }
        }),
    );

    let ws_app = Router::new()
        .route("/sync", get(ws_upgrade_handler))
        .with_state(repo);

    // Kill any stale processes before binding.
    kill_process_on_port(DOC_ID_PORT);
    kill_process_on_port(WS_PORT);

    let inf_router = Router::new()
        .route("/inference_response", post(handle_inference_response))
        .with_state(api_state);

    let http_app = Router::new()
        .merge(doc_id_app)
        .merge(inf_router);

    tokio::spawn(async move {
        let addr = SocketAddr::from(([127, 0, 0, 1], DOC_ID_PORT));
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                if let Err(err) = axum::serve(listener, http_app).await {
                    eprintln!("[Harness] doc_id server failed: {err}");
                }
            }
            Err(err) => {
                eprintln!("[Harness] bind doc_id listener: {err} — port {DOC_ID_PORT} still in use");
            }
        }
    });

    let ws_addr = SocketAddr::from(([127, 0, 0, 1], WS_PORT));
    match tokio::net::TcpListener::bind(ws_addr).await {
        Ok(ws_listener) => {
            eprintln!("[Harness] samod websocket server listening on 127.0.0.1:{WS_PORT}");
            if let Err(err) = axum::serve(ws_listener, ws_app).await {
                eprintln!("[Harness] websocket server failed: {err}");
            }
        }
        Err(err) => {
            eprintln!("[Harness] bind websocket sync listener: {err} — port {WS_PORT} still in use");
        }
    }
}

async fn handle_inference_response(
    State(state): State<ApiState>,
    Json(payload): Json<InferenceResponsePayload>,
) -> impl IntoResponse {
    eprintln!(
        "[Harness] received inference response for '{}' ({} chars)",
        payload.app_id,
        payload.content.len(),
    );

    let response_key = format!("response:{}", payload.app_id);
    let doc_handle = state.doc_handle.clone();
    doc_handle.with_doc_mut(|agent| {
        agent
            .stored_values
            .entry(response_key)
            .and_modify(|sv| sv.value = payload.content.clone())
            .or_insert_with(|| StoredValue {
                value: payload.content.clone(),
                description: format!("Inference response for {}", payload.app_id),
            });
    });

    "ok"
}

async fn ws_upgrade_handler(
    State(repo): State<Repo>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(err) = repo.accept_axum(socket) {
            eprintln!("[Harness] failed to accept websocket peer: {err:?}");
        }
    })
}

fn spawn_makepad_host() -> Option<Child> {
    let binary = find_makepad_host_binary().ok()?;

    let mut cmd = Command::new(binary);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .env("RUST_BACKTRACE", "1");

    if let Ok(windowed) = env::var("MAKEPAD_HOST_WINDOWED") {
        cmd.env("MAKEPAD_HOST_WINDOWED", windowed);
    }
    if let Ok(marker) = env::var("MAKEPAD_HOST_WINDOW_MARKER") {
        cmd.env("MAKEPAD_HOST_WINDOW_MARKER", marker);
    }

    match cmd.spawn() {
        Ok(child) => Some(child),
        Err(err) => {
            eprintln!("[Harness] failed to spawn makepad-host: {err}");
            None
        }
    }
}

fn find_makepad_host_binary() -> Result<PathBuf, String> {
    if let Ok(path) = env::var("MAKEPAD_HOST_BINARY") {
        return Ok(PathBuf::from(path));
    }

    let cwd = env::current_dir().map_err(|e| e.to_string())?;
    let candidates = [
        cwd.join("target/debug/makepad-host"),
        cwd.join("makepad-host/target/debug/makepad-host"),
    ];

    candidates
        .into_iter()
        .find(|p| is_executable(p))
        .ok_or_else(|| {
            "makepad-host binary not found. Build with `cargo build -p makepad-host` or set MAKEPAD_HOST_BINARY"
                .to_string()
        })
}

fn is_executable(path: &Path) -> bool {
    path.exists() && path.is_file()
}
