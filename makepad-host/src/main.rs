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
        // background tasks finished
    });

    // Run the Makepad app on the main thread
    app::app_main();
}

async fn background_main() {
    let repo = Repo::build_tokio().load().await;

    // Read the doc ID from env (set by harness when spawning us)
    let doc_id_str = std::env::var("MAKEPAD_HOST_DOC_ID")
        .unwrap_or_else(|_| {
            // MAKEPAD_HOST_DOC_ID not set — will discover via WS
            String::new()
        });

    let ws_url = std::env::var("MAKEPAD_HOST_WS_URL")
        .unwrap_or_else(|_| format!("ws://127.0.0.1:{SAMOD_WS_PORT}/sync"));

    // Connect to the harness's samod WS server
    // connecting to harness samod WS
    let (socket, _) = tokio_tungstenite::connect_async(ws_url.as_str())
        .await
        .expect("connect to samod WS");

    let _conn = repo
        .connect_tungstenite(socket, ConnDirection::Outgoing)
        .expect("attach websocket to samod repo");

    // connected to harness samod WS

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
                Err(_e) => {
                    // repo.find error
                    tokio::time::sleep(Duration::from_millis(CONNECT_RETRY_MS)).await;
                    continue;
                }
            }
        }
    } else {
        // No doc ID — try to discover it. This shouldn't happen in normal operation.
        // no doc ID provided, waiting for discovery...
        loop {
            // Wait for the doc to appear via sync
            tokio::time::sleep(Duration::from_millis(1000)).await;
            // Try a common pattern: discover via peer sync
            // still waiting for doc discovery...
            continue;
        }
    };

    // Store the doc handle for the Makepad thread
    if SHARED_DOC.set(doc_handle.clone()).is_err() {
        // SHARED_DOC already set — ignoring duplicate
    }

    // shared doc acquired

    // Write "ready" marker so harness knows we're up
    if let Ok(marker_path) = std::env::var("MAKEPAD_HOST_READY_MARKER") {
        let _ = std::fs::write(&marker_path, "ready\n");
    }

    // Main change listener loop
    //
    // IMPORTANT: Only signal the UI thread for changes ORIGINATED by the
    // harness (new app, debug command, exit). Changes the host itself wrote
    // to the doc (debug_response, user_response, status update) MUST NOT
    // trigger another signal — otherwise the harness clearing them creates
    // a re-entrant Signal cascade that crashes Makepad's event loop (the
    // CRDT sync fires during event processing, which intersects with
    // Makepad's internal NSTimer callbacks, causing panic_cannot_unwind).
    //
    // Track the last-known harness-originated fields and only signal when
    // a NEW value appears (current.is_some() + different from last).
    // Ignore transitions TO None (the host itself cleared the field).
    let mut changes = doc_handle.changes();
    let mut last_pending_id: Option<String> = None;
    let mut last_debug_cmd: Option<String> = None;
    let mut last_pi_response: Option<String> = None;

    while let Some(_change) = changes.next().await {
        let (should_exit, should_signal) = {
            let (current_id, current_cmd, pi_resp, exit) = doc_handle.with_document(|doc| {
                use autosurgeon::hydrate;
                let agent: AgentDoc = hydrate(doc).unwrap_or_default();
                (
                    agent.pending_app.as_ref().map(|a| a.id.clone()),
                    agent.debug_command.as_ref().map(|c| c.command.clone()),
                    agent.pi_response.clone(),
                    agent.should_exit,
                )
            });

            // Only signal when a NEW value appears (is_some guards).
            // Transitions to None (host clearing the field) are ignored.
            let signal = exit
                || (current_id.is_some() && current_id != last_pending_id)
                || (current_cmd.is_some() && current_cmd != last_debug_cmd)
                || (pi_resp.is_some() && pi_resp != last_pi_response);

            // Always update trackers so we don't re-signal for stale values
            if current_id != last_pending_id {
                last_pending_id = current_id;
            }
            if current_cmd != last_debug_cmd {
                last_debug_cmd = current_cmd;
            }
            if pi_resp != last_pi_response {
                last_pi_response = pi_resp;
            }

            (exit, signal)
        };

        if should_signal {
            SignalToUI::set_ui_signal();
        }

        if should_exit {
            // should_exit — exiting
            break;
        }
    }

    // change stream ended
}
