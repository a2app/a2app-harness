mod doc;
mod repo;
mod router;

use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use samod::Repo;
use shared::{DOC_ID_PORT, WS_PORT};

#[tokio::main]
async fn main() {
    let (repo_handle, doc_handle) = repo::start_repo().await;

    let doc_id = doc_handle.document_id().to_string();
    tokio::spawn(serve_harness_api(repo_handle.repo.clone(), doc_id));

    let _child = spawn_makepad_host();

    router::run(doc_handle).await;
}

async fn serve_harness_api(repo: Repo, doc_id: String) {
    let doc_id_app = Router::new().route(
        "/doc_id",
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

    tokio::spawn(async move {
        let addr = SocketAddr::from(([127, 0, 0, 1], DOC_ID_PORT));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("bind doc_id listener");
        if let Err(err) = axum::serve(listener, doc_id_app).await {
            eprintln!("[Harness] doc_id server failed: {err}");
        }
    });

    let ws_addr = SocketAddr::from(([127, 0, 0, 1], WS_PORT));
    let ws_listener = tokio::net::TcpListener::bind(ws_addr)
        .await
        .expect("bind websocket sync listener");
    eprintln!("[Harness] samod websocket server listening on 127.0.0.1:{WS_PORT}");
    if let Err(err) = axum::serve(ws_listener, ws_app).await {
        eprintln!("[Harness] websocket server failed: {err}");
    }
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
