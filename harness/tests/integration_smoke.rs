use std::process::{Child, Command, Stdio};
use std::str::FromStr;

use autosurgeon::{hydrate, reconcile};
use samod::{ConnDirection, DocumentId, Repo};
use shared::{AppStatus, PendingApp, WS_PORT};
use tempfile::TempDir;
use tokio::time::{sleep, timeout, Duration, Instant};

const SMOKE_APP_ID: &str = "smoke-todo";
const TODO_SPLASH_BODY: &str = r#"RoundedView{
    width: Fill height: Fit
    flow: Down spacing: 10
    padding: 16
    draw_bg.color: #x1e1e2e
    draw_bg.border_radius: 10.0
    Label{text: "Todo" draw_text.color: #fff}
}"#;

// ── Smoke test: Rust client sets pending_app, waits for Launched ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smoke_launches_splash_app_via_pending_app() {
    let temp_home = TempDir::new().expect("create temp HOME for isolated repo state");
    let harness_bin =
        std::env::var("CARGO_BIN_EXE_harness").expect("cargo test should provide harness binary");

    let mut harness = Command::new(&harness_bin)
        .env("HOME", temp_home.path())
        .env("MAKEPAD_HOST_DISABLE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn harness process");

    let result = run_smoke_client_flow().await;

    request_harness_shutdown().await;
    terminate_child(&mut harness);

    result.expect("integration smoke flow should succeed");
}

async fn run_smoke_client_flow() -> Result<(), String> {
    let doc_id = wait_for_doc_id(Duration::from_secs(20)).await?;

    let repo = Repo::build_tokio().load().await;
    let ws_url = format!("ws://127.0.0.1:{WS_PORT}/sync");
    let (socket, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|e| format!("connect websocket: {e}"))?;

    let connection = repo
        .connect_tungstenite(socket, ConnDirection::Outgoing)
        .map_err(|e| format!("attach websocket to samod repo: {e:?}"))?;
    connection
        .handshake_complete()
        .await
        .map_err(|e| format!("samod handshake failed: {e:?}"))?;

    let parsed_doc_id =
        DocumentId::from_str(doc_id.trim()).map_err(|e| format!("parse doc id '{doc_id}': {e}"))?;

    let doc_handle = wait_for_doc_handle(&repo, parsed_doc_id).await?;

    // Set pending_app with status Pending + extension_requests flag
    doc_handle.with_document(|doc| {
        let mut agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
        agent.pending_app = Some(PendingApp {
            id: SMOKE_APP_ID.to_string(),
            splash_body: TODO_SPLASH_BODY.to_string(),
            status: AppStatus::Pending,
        });
        agent.extension_requests = true;
        let mut tx = doc.transaction();
        reconcile(&mut tx, &agent).expect("reconcile");
        tx.commit();
    });

    // Wait for the host to set status to Launched
    wait_for_app_launched(&doc_handle).await
}

async fn wait_for_app_launched(doc_handle: &samod::DocHandle) -> Result<(), String> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        let launched = doc_handle.with_document(|doc| {
            let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
            match agent.pending_app {
                Some(ref app) => {
                    app.id == SMOKE_APP_ID && app.status == AppStatus::Launched
                }
                None => false,
            }
        });

        if launched {
            return Ok(());
        }

        sleep(Duration::from_millis(100)).await;
    }

    Err("timed out waiting for pending_app status to become Launched".to_string())
}

// ── Helpers ──────────────────────────────────────────────────────────────

async fn wait_for_doc_id(max_wait: Duration) -> Result<String, String> {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{WS_PORT}/doc_id");
    let started = Instant::now();

    while started.elapsed() < max_wait {
        match client.get(&url).send().await {
            Ok(resp) => match resp.text().await {
                Ok(body) if !body.trim().is_empty() => return Ok(body),
                Ok(_) => {}
                Err(err) => return Err(format!("read doc_id body: {err}")),
            },
            Err(_) => {}
        }
        sleep(Duration::from_millis(100)).await;
    }

    Err("timed out waiting for /doc_id".to_string())
}

async fn wait_for_doc_handle(
    repo: &Repo,
    doc_id: DocumentId,
) -> Result<samod::DocHandle, String> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        match repo.find(doc_id.clone()).await {
            Ok(Some(handle)) => return Ok(handle),
            Ok(None) => sleep(Duration::from_millis(100)).await,
            Err(err) => return Err(format!("repo.find failed: {err:?}")),
        }
    }

    Err("timed out waiting for shared document handle".to_string())
}

async fn request_harness_shutdown() {
    let repo = Repo::build_tokio().load().await;
    let ws_url = format!("ws://127.0.0.1:{WS_PORT}/sync");

    let connect = timeout(Duration::from_secs(2), tokio_tungstenite::connect_async(&ws_url)).await;
    let Ok(Ok((socket, _))) = connect else {
        return;
    };

    let Ok(connection) = repo.connect_tungstenite(socket, ConnDirection::Outgoing) else {
        return;
    };

    if connection.handshake_complete().await.is_err() {
        return;
    }

    let Ok(doc_id) = wait_for_doc_id(Duration::from_secs(2)).await else {
        return;
    };
    let Ok(doc_id) = DocumentId::from_str(doc_id.trim()) else {
        return;
    };
    let Ok(doc_handle) = wait_for_doc_handle(&repo, doc_id).await else {
        return;
    };

    doc_handle.with_document(|doc| {
        let mut agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
        agent.should_exit = true;
        let mut tx = doc.transaction();
        let _ = reconcile(&mut tx, &agent);
        tx.commit();
    });
}

fn terminate_child(child: &mut Child) {
    if let Ok(Some(_)) = child.try_wait() {
        return;
    }
    let _ = child.kill();
    let _ = child.wait();
}
