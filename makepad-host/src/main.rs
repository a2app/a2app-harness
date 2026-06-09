use std::sync::OnceLock;
use std::time::Duration;

use futures::StreamExt;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use samod::{ConnDirection, DocHandle, Repo};
use shared::AgentDoc;
use tokio::runtime::Runtime;

mod agent_splash;
mod app;

/// Shared doc handle — set once by the background async thread,
/// read/written by the Makepad main thread (for send_response)
/// and the background thread (for the change listener).
pub static SHARED_DOC: OnceLock<DocHandle> = OnceLock::new();

const SAMOD_WS_PORT: u16 = 2342;
const CONNECT_RETRY_MS: u64 = 500;

fn main() {
    // Start the background tokio runtime on a separate thread.
    // This thread:
    //   - Connects to the harness's samod WS server
    //   - Finds the shared document
    //   - Listens for doc changes → signals the Makepad main thread
    //   - Handles should_exit
    std::thread::spawn(|| {
        let rt = Runtime::new().expect("create tokio runtime");
        rt.block_on(background_main());
        eprintln!("[makepad-host] background tasks finished");
    });

    // Run the Makepad app on the main thread
    app::app_main();
}

async fn background_main() {
    let repo = Repo::build_tokio().load().await;

    // Read the doc ID from env (set by harness when spawning us)
    let doc_id_str = std::env::var("MAKEPAD_HOST_DOC_ID")
        .unwrap_or_else(|_| {
            eprintln!("[makepad-host] MAKEPAD_HOST_DOC_ID not set — will discover via WS");
            String::new()
        });

    let ws_url = std::env::var("MAKEPAD_HOST_WS_URL")
        .unwrap_or_else(|_| format!("ws://127.0.0.1:{SAMOD_WS_PORT}/sync"));

    // Connect to the harness's samod WS server
    eprintln!("[makepad-host] connecting to {}...", ws_url);
    let (socket, _) = tokio_tungstenite::connect_async(ws_url.as_str())
        .await
        .expect("connect to samod WS");

    let _conn = repo
        .connect_tungstenite(socket, ConnDirection::Outgoing)
        .expect("attach websocket to samod repo");

    eprintln!("[makepad-host] connected to harness samod WS");

    // Find the shared document
    let doc_handle = if !doc_id_str.is_empty() {
        let parsed_id: samod::DocumentId = doc_id_str.parse().expect("parse doc ID");
        loop {
            match repo.find(parsed_id.clone()).await {
                Ok(Some(handle)) => break handle,
                Ok(None) => {
                    tokio::time::sleep(Duration::from_millis(CONNECT_RETRY_MS)).await;
                    continue;
                }
                Err(e) => {
                    eprintln!("[makepad-host] repo.find error: {e:?}");
                    tokio::time::sleep(Duration::from_millis(CONNECT_RETRY_MS)).await;
                    continue;
                }
            }
        }
    } else {
        // No doc ID — try to discover it. This shouldn't happen in normal operation.
        eprintln!("[makepad-host] no doc ID provided, waiting for discovery...");
        loop {
            // Wait for the doc to appear via sync
            tokio::time::sleep(Duration::from_millis(1000)).await;
            // Try a common pattern: discover via peer sync
            eprintln!("[makepad-host] still waiting for doc discovery...");
            continue;
        }
    };

    // Store the doc handle for the Makepad thread
    if SHARED_DOC.set(doc_handle.clone()).is_err() {
        eprintln!("[makepad-host] SHARED_DOC already set — ignoring duplicate");
    }

    eprintln!("[makepad-host] shared doc acquired");

    // Write "ready" marker so harness knows we're up
    if let Ok(marker_path) = std::env::var("MAKEPAD_HOST_READY_MARKER") {
        let _ = std::fs::write(&marker_path, "ready\n");
    }

    // Main change listener loop
    // When the doc changes (new app from harness, user_response from AgentSplash),
    // set DOC_CHANGED flag so the Makepad main thread picks it up on next Draw.
    let mut changes = doc_handle.changes();
    while let Some(_change) = changes.next().await {
        let should_exit = doc_handle.with_document(|doc| {
            use autosurgeon::hydrate;
            let agent: AgentDoc = hydrate(doc).unwrap_or_default();
            eprintln!(
                "[makepad-host] change: id={:?} exit={}",
                agent.pending_app.as_ref().map(|a| &a.id),
                agent.should_exit,
            );
            agent.should_exit
        });

        // Signal the Makepad main thread to re-sync
        SignalToUI::set_ui_signal();

        if should_exit {
            eprintln!("[makepad-host] should_exit — exiting");
            break;
        }
    }

    eprintln!("[makepad-host] change stream ended");
}
