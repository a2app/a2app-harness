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
    #[serde(rename = "pi_response")]
    PiResponse { app_id: String, data: String },
    #[serde(rename = "clear")]
    Clear { app_id: String },
    #[serde(rename = "debug")]
    Debug { app_id: String, command: String, params: String },
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
    let _ = tracing_subscriber::fmt::try_init();

    let headless = env::var("HARNESS_HEADLESS").ok().as_deref() == Some("1");

    std::thread::spawn(move || {
        let rt = Runtime::new().expect("create tokio runtime");
        rt.block_on(background_main(headless));
    });

    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

// ── Background: bridge logic ─────────────────────────────────────────────

async fn background_main(headless: bool) {
    let repo = samod::Repo::build_tokio().load().await;

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
        agent.pi_response = None;
        let mut tx = doc.transaction();
        reconcile(&mut tx, &agent).expect("reconcile");
        tx.commit();
    });

    let doc_id = doc_handle.document_id().to_string();

    // ── 2. Set up samod WS server for makepad-host ─────────────────
    let repo_clone = repo.clone();
    tokio::spawn(async move {
        let ws_app = Router::new()
            .route("/sync", get(move |ws: WebSocketUpgrade| {
                let repo = repo_clone.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        if let Err(e) = repo.accept_axum(socket) {
                            eprintln!("accept makepad-host WS: {e:?}");
                        }
                    })
                }
            }));
        let addr = SocketAddr::from(([127, 0, 0, 1], SAMOD_WS_PORT));
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                let _ = axum::serve(listener, ws_app).await;
            }
            Err(e) => eprintln!("samod WS bind: {e}"),
        }
    });

    // ── 3. Spawn makepad-host (unless headless) ──────────────────────
    let mut makepad_child: Option<Child> = None;
    if !headless {
        let ready_marker = format!("/tmp/makepad-host-ready-{}.marker", std::process::id());
        let _ = std::fs::remove_file(&ready_marker);

        let harness_bin = env::current_exe().ok();
        let makepad_bin = if let Some(ref bin) = harness_bin {
            let mut p = bin.parent().unwrap().to_path_buf();
            p.push("makepad-host");
            if p.exists() { p } else {
                let mut p = bin.parent().unwrap().parent().unwrap().to_path_buf();
                p.push("makepad-host");
                p.push("target");
                p.push("debug");
                p.push("makepad-host");
                p
            }
        } else {
            let mut p = env::current_dir().unwrap_or_default();
            p.push("target");
            p.push("debug");
            p.push("makepad-host");
            p
        };

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
                if !ready {
                    eprintln!("WARNING: makepad-host did not become ready within timeout");
                }
                let _ = std::fs::remove_file(&ready_marker);
            }
            Err(e) => {
                eprintln!("failed to spawn makepad-host: {e}");
                eprintln!("continuing without makepad-host (headless-like mode)");
            }
        }
    }

    // ── 4. Set up JSON WS server for pi extension ────────────────────
    use tokio::sync::watch;
    let (fwd_tx, fwd_rx) = watch::channel(String::new());
    let pending_interaction = Arc::new(AtomicBool::new(false));
    let bridge_state = BridgeState {
        doc: doc_handle.clone(),
        fwd_tx: fwd_tx.clone(),
        fwd_rx: tokio::sync::Mutex::new(fwd_rx),
        pending_interaction: pending_interaction.clone(),
    };
    let bridge = std::sync::Arc::new(tokio::sync::Mutex::new(bridge_state));

    let bridge_for_ws = bridge.clone();
    let doc_id_for_http = doc_id.clone();
    tokio::spawn(async move {
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
        kill_process_on_port(JSON_WS_PORT);
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                let _ = axum::serve(listener, json_app).await;
            }
            Err(e) => eprintln!("JSON WS bind: {e}"),
        }
    });

    // ── 5. Bridge loop: doc changes → push to pi ────────────────────
    let mut doc_changes = doc_handle.changes();
    let mut last_user_response_version: u64 = 0;
    let mut last_error_message: Option<String> = None;
    let mut last_status: Option<String> = None;
    let mut last_status_app_id: Option<String> = None;

    while let Some(_change) = doc_changes.next().await {
        if pending_interaction.swap(false, Ordering::SeqCst) {
            continue;
        }

        let (has_response, version, app_id, status, error_message, debug_response, exit) = doc_handle.with_document(|doc| {
            use autosurgeon::hydrate;
            let agent: AgentDoc = hydrate(doc).unwrap_or_default();
            let app_id = agent.pending_app.as_ref().map(|a| a.id.clone());
            let status = agent.pending_app.as_ref().map(|a| match &a.status {
                shared::AppStatus::Pending => "Pending".to_string(),
                shared::AppStatus::Launched => "Launched".to_string(),
            });
            (agent.user_response.clone(), agent.user_response_version, app_id, status, agent.error_message.clone(), agent.debug_response.clone(), agent.should_exit)
        });

        if version != 0 && version != last_user_response_version {
            if let Some(ref response) = has_response {
                if let Some(ref id) = app_id {
                    let msg = HarnessToPiMsg::UserResponse {
                        app_id: id.clone(),
                        response: response.clone(),
                    };
                    let json = serde_json::to_string(&msg).unwrap_or_default();
                    let _ = bridge.lock().await.fwd_tx.send(json);
                }
            }
            last_user_response_version = version;
        }

        if let Some(ref result) = debug_response {
            if let Some(ref id) = app_id {
                let msg = HarnessToPiMsg::DebugResponse {
                    app_id: id.clone(),
                    result: result.clone(),
                };
                let json = serde_json::to_string(&msg).unwrap_or_default();
                let _ = bridge.lock().await.fwd_tx.send(json);
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

        if status != last_status || app_id != last_status_app_id {
            if let Some(ref id) = app_id {
                if let Some(ref st) = status {
                    let msg = HarnessToPiMsg::Status {
                        app_id: id.clone(),
                        status: st.clone(),
                    };
                    let json = serde_json::to_string(&msg).unwrap_or_default();
                    let _ = bridge.lock().await.fwd_tx.send(json);
                }
            }
            last_status = status;
            last_status_app_id = app_id.clone();
        }

        if error_message != last_error_message {
            if let Some(ref msg_text) = error_message {
                if let Some(ref id) = app_id {
                    let msg = HarnessToPiMsg::Error {
                        app_id: id.clone(),
                        message: msg_text.clone(),
                    };
                    let json = serde_json::to_string(&msg).unwrap_or_default();
                    let _ = bridge.lock().await.fwd_tx.send(json);
                }
            }
            last_error_message = error_message;
        }

        if exit {
            break;
        }
    }

    if let Some(mut child) = makepad_child {
        let _ = child.kill();
        let _ = child.wait();
    }
}

// ── Bridge state ─────────────────────────────────────────────────────────

use tokio::sync::mpsc;

struct BridgeState {
    doc: DocHandle,
    fwd_tx: mpsc::UnboundedSender<String>,
    fwd_rx: tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<String>>>,
    pending_interaction: Arc<AtomicBool>,
}

// ── Handle pi WebSocket connection ───────────────────────────────────────

async fn handle_pi_ws(ws: WebSocket, bridge: std::sync::Arc<tokio::sync::Mutex<BridgeState>>) {
    let (mut ws_tx, mut ws_rx) = ws.split();

    // Take the global fwd_rx from the bridge (created in background_main)
    let mut fwd_rx = bridge.lock().await.fwd_rx.lock().await.take().expect("fwd_rx already taken");

    let fwd_handle = tokio::spawn(async move {
        while let Some(msg) = fwd_rx.recv().await {
            if ws_tx.send(Message::Text(axum::extract::ws::Utf8Bytes::from(msg))).await.is_err() {
                break;
            }
        }
    });

    // Send welcome using the bridge's fwd_tx
    let welcome = serde_json::to_string(&HarnessToPiMsg::Welcome).unwrap_or_default();
    let _ = bridge.lock().await.fwd_tx.send(welcome);

    let doc_handle = bridge.lock().await.doc.clone();
    let fwd_tx = bridge.lock().await.fwd_tx.clone();
    while let Some(Ok(msg)) = ws_rx.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Binary(_) => continue,
            Message::Close(_) | Message::Ping(_) | Message::Pong(_) => continue,
        };

        let parsed: PiToHarnessMsg = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("bad JSON from pi: {e}");
                continue;
            }
        };

        match parsed {
            PiToHarnessMsg::Launch { app_id, splash_body } => {
                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
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
                if command == "click" || command == "type_text" {
                    bridge.lock().await.pending_interaction.store(true, Ordering::SeqCst);
                }

                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
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
                        pi_response: agent.pi_response,
                        error_message: agent.error_message,
                        status,
                    };
                    let json = serde_json::to_string(&msg).unwrap_or_default();
                    let _ = fwd_tx.send(json);
                });
            }
            PiToHarnessMsg::PiResponse { app_id, data } => {
                doc_handle.with_document(|doc| {
                    use autosurgeon::{hydrate, reconcile};
                    let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
                    agent.pi_response = Some(data);
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                });
            }
            PiToHarnessMsg::Clear { app_id } => {
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
                    eprintln!("Killing stale process {pid} on port {port}");
                    let _ = Command::new("kill").args(["-9", pid]).status();
                }
            }
        }
    }
}
