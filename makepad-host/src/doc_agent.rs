use std::collections::HashMap;
use std::str::FromStr;

use autosurgeon::{hydrate, reconcile};
use futures::StreamExt;
use reqwest::Client;
use samod::{ConnDirection, Connection, DocHandle as SamodDocHandle, Repo};
use samod_core::DocumentId;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration, Instant};

use shared::{AgentDoc, AgentRequest, AgentResponse, StoredValue, DOC_ID_PORT, WS_PORT};

use crate::state::{get_host_state, get_signal, AppState, HostCommand, PendingLaunch};

pub struct DocSession {
    #[allow(dead_code)]
    pub repo: Repo,
    #[allow(dead_code)]
    pub connection: Connection,
    pub doc_handle: DocHandle,
}

#[derive(Clone)]
pub struct DocHandle {
    inner: SamodDocHandle,
}

impl DocHandle {
    pub fn with_doc<T>(&self, f: impl FnOnce(&AgentDoc) -> T) -> T {
        self.inner.with_document(|doc| {
            let agent: AgentDoc = hydrate(doc).unwrap_or_default();
            f(&agent)
        })
    }

    pub fn with_doc_mut<T>(&self, f: impl FnOnce(&mut AgentDoc) -> T) -> T {
        self.inner.with_document(|doc| {
            let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
            let out = f(&mut agent);
            let mut tx = doc.transaction();
            reconcile(&mut tx, &agent).expect("reconcile agent doc");
            tx.commit();
            out
        })
    }

    pub fn changes(&self) -> impl futures::Stream<Item = samod_core::DocumentChanged> {
        self.inner.changes()
    }
}

pub async fn setup_doc() -> DocSession {
    let repo = Repo::build_tokio().load().await;

    // Wait for the harness HTTP API to be up first
    let doc_id_str = wait_for_doc_id(&format!("http://127.0.0.1:{DOC_ID_PORT}/doc_id")).await;
    let doc_id = DocumentId::from_str(doc_id_str.trim()).expect("parse harness doc_id");

    // Retry websocket connection until the harness is ready
    let ws_url = format!("ws://127.0.0.1:{WS_PORT}/sync");
    let connection = loop {
        match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((socket, _)) => {
                match repo.connect_tungstenite(socket, ConnDirection::Outgoing) {
                    Ok(conn) => break conn,
                    Err(err) => {
                        eprintln!("[doc_agent] attach websocket failed: {err:?}, retrying...");
                        sleep(Duration::from_millis(200)).await;
                    }
                }
            }
            Err(err) => {
                eprintln!("[doc_agent] connect websocket failed: {err:?}, retrying...");
                sleep(Duration::from_millis(200)).await;
            }
        }
    };
    connection
        .handshake_complete()
        .await
        .expect("samod websocket handshake");

    let doc_handle = loop {
        match repo.find(doc_id.clone()).await {
            Ok(Some(handle)) => break handle,
            Ok(None) => sleep(Duration::from_millis(100)).await,
            Err(err) => panic!("failed to find shared document: {err:?}"),
        }
    };

    DocSession {
        repo,
        connection,
        doc_handle: DocHandle { inner: doc_handle },
    }
}

pub async fn run(session: DocSession, mut cmd_rx: mpsc::UnboundedReceiver<HostCommand>) {
    let doc_handle = session.doc_handle.clone();
    handle_doc_change(&doc_handle).await;
    let mut changes = doc_handle.changes();

    loop {
        tokio::select! {
            Some(_changed) = changes.next() => {
                handle_doc_change(&doc_handle).await;
            }
            Some(cmd) = cmd_rx.recv() => {
                handle_host_command(&doc_handle, cmd).await;
            }
            else => break,
        }
    }
}

async fn handle_doc_change(doc_handle: &DocHandle) {
    let (should_exit, new_apps, closed_apps, inference_results, stored_results) = doc_handle.with_doc(|agent| {
        let state = get_host_state();
        let state_guard = state.read().expect("host state poisoned");

        let new: Vec<(String, String)> = agent
            .mini_apps
            .iter()
            .filter(|(id, _)| !state_guard.apps.contains_key(*id))
            .map(|(id, app)| (id.clone(), app.splash_body.clone()))
            .collect();

        // NEVER auto-close apps based on doc state absence.
        // The doc may temporarily appear empty during sync, which would
        // incorrectly close all running apps. Apps are only closed via
        // explicit CloseApp requests processed in router.rs.
        let closed: Vec<String> = Vec::new();

        let results: Vec<(String, String)> = agent
            .responses
            .iter()
            .filter_map(|r| match r {
                AgentResponse::InferenceResult { app_id, content } => {
                    Some((app_id.clone(), content.clone()))
                }
                _ => None,
            })
            .collect();

        // HTTP fallback: read inference responses from stored_values
        // (written by the harness's /inference_response endpoint)
        let mut stored: HashMap<String, String> = HashMap::new();
        for (key, sv) in &agent.stored_values {
            if let Some(app_id) = key.strip_prefix("response:") {
                stored.insert(app_id.to_string(), sv.value.clone());
            }
        }

        (agent.should_exit, new, closed, results, stored)
    });

    if should_exit {
        std::process::exit(0);
    }

    if let Some(signal) = get_signal() {
        for (id, splash_body) in new_apps {
            let state = get_host_state();
            let mut state = state.write().expect("host state poisoned");
            if !state.app_order.iter().any(|existing| existing == &id) {
                state.app_order.push(id.clone());
            }
            state
                .apps
                .entry(id.clone())
                .and_modify(|app| app.content = splash_body.clone())
                .or_insert_with(|| AppState::new(splash_body.clone()));
            state.pending_launches.push(PendingLaunch {
                id: id.clone(),
                content: splash_body,
            });
            // Newly launched app becomes the active one.
            state.active_app_id = Some(id);
            state.bump_revision();
            signal.set();
        }

        for id in closed_apps {
            let state = get_host_state();
            let mut state = state.write().expect("host state poisoned");
            state.apps.remove(&id);
            state.app_order.retain(|x| x != &id);
            state.ensure_active_app();
            state.bump_revision();
            signal.set();
        }

        // Process both direct responses and HTTP fallback stored_values
        for (app_id, content) in inference_results.into_iter().chain(
            stored_results.into_iter().map(|(k, v)| (k, v))
        ) {
            let state = get_host_state();
            let mut state = state.write().expect("host state poisoned");
            state
                .apps
                .entry(app_id.clone())
                .and_modify(|app| {
                    app.last_response = Some(content.clone());
                    app.request_in_flight = false;
                    if let Some(tx) = app.pending_inference.pop_front() {
                        let _ = tx.send(content.clone());
                    }
                })
                .or_insert_with(|| {
                    let mut app = AppState::new(String::new());
                    app.last_response = Some(content.clone());
                    app
                });
            state.bump_revision();
            signal.set();

            doc_handle.with_doc_mut(|agent| {
                // Clean up direct response
                let pos = agent.responses.iter().position(|r| {
                    matches!(r, AgentResponse::InferenceResult { app_id: id, .. } if id == &app_id)
                });
                if let Some(i) = pos {
                    agent.responses.remove(i);
                }
                // Clean up HTTP fallback stored_value
                agent.stored_values.remove(&format!("response:{}", app_id));
            });
        }
    }
}

async fn handle_host_command(doc_handle: &DocHandle, cmd: HostCommand) {
    match cmd {
        HostCommand::Inference { app_id, content, .. } => {
            {
                let state = get_host_state();
                let mut state = state.write().expect("host state poisoned");
                if let Some(app) = state.apps.get_mut(&app_id) {
                    app.last_request = Some(content.clone());
                    app.request_in_flight = true;
                    state.bump_revision();
                }
            }
            doc_handle.with_doc_mut(|agent| {
                agent
                    .requests
                    .push(AgentRequest::Inference { content, app_id });
            });
        }
        HostCommand::CloseApp(id) => {
            doc_handle.with_doc_mut(|agent| {
                agent.requests.push(AgentRequest::CloseApp { id });
            });
        }
    }
}

async fn wait_for_doc_id(url: &str) -> String {
    let client = Client::new();
    let deadline = Instant::now() + Duration::from_secs(30);

    loop {
        if Instant::now() > deadline {
            panic!("timed out waiting for doc id from {url}");
        }

        if let Ok(resp) = client.get(url).send().await
            && let Ok(text) = resp.text().await
        {
            let value = text.trim().to_string();
            if !value.is_empty() {
                return value;
            }
        }

        sleep(Duration::from_millis(250)).await;
    }
}
