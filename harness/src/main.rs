mod doc;
mod repo;
mod router;

use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use axum::routing::get;
use axum::Router;
use shared::DOC_ID_PORT;

#[tokio::main]
async fn main() {
    let (_repo_handle, doc_handle) = repo::start_repo().await;

    let doc_id = doc_handle.document_id().to_string();
    tokio::spawn(serve_doc_id(doc_id, DOC_ID_PORT));

    let _child = spawn_makepad_host();

    router::run(doc_handle).await;
}

async fn serve_doc_id(doc_id: String, port: u16) {
    let app = Router::new().route(
        "/doc_id",
        get(move || {
            let body = doc_id.clone();
            async move { body }
        }),
    );

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    if let Err(err) = axum::serve(
        tokio::net::TcpListener::bind(addr)
            .await
            .expect("bind doc id listener"),
        app,
    )
    .await
    {
        eprintln!("[Harness] doc_id server failed: {err}");
    }
}

fn spawn_makepad_host() -> Option<Child> {
    let binary = find_makepad_host_binary().ok()?;

    let mut cmd = Command::new(binary);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .env("RUST_BACKTRACE", "1");

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
