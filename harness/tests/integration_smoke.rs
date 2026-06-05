use std::process::{Child, Command, Stdio};
use std::str::FromStr;

use autosurgeon::{hydrate, reconcile};
use samod::{ConnDirection, DocumentId, Repo};
use shared::{AppStatus, PendingApp, JSON_WS_PORT, SAMOD_WS_PORT};
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

// ── Smoke test ───────────────────────────────────────────────────────────
// Connects to the harness's samod WS (port 2342), finds the shared doc,
// writes to it, and verifies local readback.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smoke_basic_connect_and_sync() {
    let temp_home = TempDir::new().expect("create temp HOME");
    let harness_bin =
        std::env::var("CARGO_BIN_EXE_harness").expect("cargo test should provide harness binary");

    let mut harness = Command::new(&harness_bin)
        .env("HOME", temp_home.path())
        .env("HARNESS_HEADLESS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn harness process");

    let result = run_smoke_client_flow().await;
    terminate_child(&mut harness);
    result.expect("smoke flow should succeed");
}

async fn run_smoke_client_flow() -> Result<(), String> {
    // First, discover the doc ID via the JSON WS HTTP endpoint
    let doc_id_str = wait_for_doc_id(Duration::from_secs(20)).await?;
    let doc_id = DocumentId::from_str(doc_id_str.trim())
        .map_err(|e| format!("parse doc id '{doc_id_str}': {e}"))?;

    // Connect to the samod WS (port 2342, /sync)
    let repo = Repo::build_tokio().load().await;
    let ws_url = format!("ws://127.0.0.1:{SAMOD_WS_PORT}/sync");
    let (socket, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|e| format!("connect websocket: {e}"))?;

    let _conn = repo
        .connect_tungstenite(socket, ConnDirection::Outgoing)
        .map_err(|e| format!("attach websocket to samod repo: {e:?}"))?;

    // Wait for handshake and document sync
    sleep(Duration::from_millis(1000)).await;

    let doc_handle = wait_for_doc_handle(&repo, doc_id).await?;

    // Step 1: Write pending_app + extension_requests
    eprintln!("[test] writing pending_app + extension_requests = true");
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

    // Step 2: Verify we can read back our own write (local read)
    let verify = doc_handle.with_document(|doc| {
        let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
        (
            agent.extension_requests,
            agent.pending_app.as_ref().map(|a| a.status.clone()),
        )
    });
    assert_eq!(verify.0, true, "we should see our own extension_requests=true");
    assert_eq!(verify.1, Some(AppStatus::Pending), "we should see Pending status");
    eprintln!("[test] local write verified OK");

    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────

async fn wait_for_doc_id(max_wait: Duration) -> Result<String, String> {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{JSON_WS_PORT}/doc_id");
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

fn terminate_child(child: &mut Child) {
    if let Ok(Some(_)) = child.try_wait() {
        return;
    }
    let _ = child.kill();
    let _ = child.wait();
}
