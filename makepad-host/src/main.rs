use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use samod::{ConnDirection, DocHandle, Repo};
use shared::AgentDoc;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

mod agent_splash;
mod app;

/// Shared doc handle — set once by the background async thread,
/// read/written by the Makepad main thread (for send_response)
/// and the background thread (for the change listener).
pub static SHARED_DOC: OnceLock<DocHandle> = OnceLock::new();

/// Streaming channel: background thread sends deltas, UI thread receives them.
/// This bypasses CRDT polling and delivers each delta as a separate event,
/// exactly like AgentEvent::TextDelta in the aichat example.
pub static STREAMING_RX: OnceLock<Mutex<mpsc::UnboundedReceiver<String>>> = OnceLock::new();

/// The last panic backtrace captured by the panic hook.
/// Cleared after the catch_unwind handler reads it.
pub static LAST_PANIC_BACKTRACE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

const SAMOD_WS_PORT: u16 = 2342;
const CONNECT_RETRY_MS: u64 = 500;

fn main() {
    // Install a panic hook to capture backtraces for the CRDT doc.
    // Must be set before any thread spawns.
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Capture backtrace
        let bt = std::backtrace::Backtrace::capture();
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown".to_string()
        };
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let backtrace_str = format!(
            "PANIC: {}\nlocation: {}\nbacktrace:\n{}",
            msg, location, bt
        );
        // Store in global so catch_unwind handler can write to CRDT doc
        if let Some(global) = LAST_PANIC_BACKTRACE.get() {
            if let Ok(mut guard) = global.lock() {
                *guard = Some(backtrace_str.clone());
            }
        }
        // Also print to stderr
        eprintln!("[panic hook] {}", backtrace_str);
        // Call original hook to preserve default behavior
        orig_hook(info);
    }));

    // Initialize the LAST_PANIC_BACKTRACE global
    LAST_PANIC_BACKTRACE.set(Mutex::new(None)).ok();

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
    let mut last_streaming_text: Option<String> = None;

    // Use a poll-based approach: check the doc every 500ms AND listen for changes.
    // This ensures we don't miss remote changes that the change listener might skip.
    //
    // Streaming deltas: instead of relying on Signal coalescing (which batches
    // rapid CRDT changes), we send each delta through a lock-free mpsc channel.
    // The UI thread reads from this channel in AgentSplash.handle_event, exactly
    // like how AgentEvent::TextDelta works in the aichat example.
    let (delta_tx, delta_rx) = mpsc::unbounded_channel::<String>();
    STREAMING_RX.set(Mutex::new(delta_rx)).ok();

    loop {
        // Wait for a change OR timeout every 500ms
        {
            use tokio::time::timeout;
            match timeout(Duration::from_millis(500), changes.next()).await {
                Ok(Some(_change)) => {} // Got a change notification
                Ok(None) => break,      // Stream ended
                Err(_) => {}            // Timeout — poll
            }
        };
        
        let (current_id, current_cmd, pi_resp, streaming, should_exit) = doc_handle.with_document(|doc| {
            use autosurgeon::hydrate;
            let agent: AgentDoc = hydrate(doc).unwrap_or_default();
            (
                agent.pending_app.as_ref().map(|a| a.id.clone()),
                agent.debug_command.as_ref().map(|c| c.command.clone()),
                agent.pi_response.clone(),
                agent.streaming_text.clone(),
                agent.should_exit,
            )
        });

        // Only signal when a NEW value appears (is_some guards).
        // Transitions to None (host clearing the field) are ignored.
        let should_signal = should_exit
            || (current_id.is_some() && current_id != last_pending_id)
            || (current_cmd.is_some() && current_cmd != last_debug_cmd)
            || (pi_resp.is_some() && pi_resp != last_pi_response)
            || (streaming.is_some() && streaming != last_streaming_text);

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
        if streaming != last_streaming_text {
            // Send delta through the channel so the UI thread can process it
            // immediately, without waiting for Signal coalescing.
            if let Some(ref text) = streaming {
                // Don't send the very first empty/blank text
                if !text.is_empty() {
                    let _ = delta_tx.send(text.clone());
                }
            }
            last_streaming_text = streaming;
        }

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
