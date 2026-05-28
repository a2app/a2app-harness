use std::collections::HashSet;
use std::sync::Arc;

use reqwest::Client;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::time::{sleep, Duration, Instant};

use shared::{AgentDoc, AgentRequest, AgentResponse, DOC_ID_PORT, WS_PORT};

use crate::state::{get_host_state, get_signal, AppState, HostCommand, PendingLaunch};

#[derive(Clone)]
pub struct DocHandle {
    #[allow(dead_code)]
    pub id: String,
    inner: Arc<RwLock<AgentDoc>>,
    changed_tx: broadcast::Sender<()>,
}

impl DocHandle {
    pub fn with_doc<T>(&self, f: impl FnOnce(&AgentDoc) -> T) -> T {
        let guard = self.inner.blocking_read();
        f(&guard)
    }

    pub fn with_doc_mut<T>(&self, f: impl FnOnce(&mut AgentDoc) -> T) -> T {
        let mut guard = self.inner.blocking_write();
        let out = f(&mut guard);
        let _ = self.changed_tx.send(());
        out
    }

    pub async fn changed(&self) -> Result<(), broadcast::error::RecvError> {
        let mut rx = self.changed_tx.subscribe();
        rx.recv().await
    }
}

pub async fn setup_doc() -> DocHandle {
    let doc_id = wait_for_doc_id(&format!("http://127.0.0.1:{DOC_ID_PORT}/doc_id")).await;
    eprintln!("[makepad-host] connect ws://127.0.0.1:{WS_PORT} for doc {doc_id}");

    let (changed_tx, _) = broadcast::channel(256);
    DocHandle {
        id: doc_id,
        inner: Arc::new(RwLock::new(AgentDoc::default())),
        changed_tx,
    }
}

pub async fn run(doc_handle: DocHandle, mut cmd_rx: mpsc::UnboundedReceiver<HostCommand>) {
    loop {
        tokio::select! {
            changed = doc_handle.changed() => {
                if changed.is_err() {
                    break;
                }
                handle_doc_change(&doc_handle).await;
            }
            Some(cmd) = cmd_rx.recv() => {
                handle_host_command(&doc_handle, cmd).await;
            }
        }
    }
}

async fn handle_doc_change(doc_handle: &DocHandle) {
    let (should_exit, new_apps, closed_apps, inference_results) = doc_handle.with_doc(|agent| {
        let state = get_host_state();
        let state_guard = state.read().expect("host state poisoned");

        let new: Vec<(String, String)> = agent
            .mini_apps
            .iter()
            .filter(|(id, _)| !state_guard.apps.contains_key(*id))
            .map(|(id, app)| (id.clone(), app.splash_body.clone()))
            .collect();

        let active_ids: HashSet<String> = agent.mini_apps.keys().cloned().collect();
        let closed: Vec<String> = state_guard
            .apps
            .keys()
            .filter(|id| !active_ids.contains(*id))
            .cloned()
            .collect();

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

        (agent.should_exit, new, closed, results)
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
                id,
                content: splash_body,
            });
            state.bump_revision();
            signal.set();
        }

        for id in closed_apps {
            let state = get_host_state();
            let mut state = state.write().expect("host state poisoned");
            state.apps.remove(&id);
            state.app_order.retain(|x| x != &id);
            state.bump_revision();
            signal.set();
        }

        for (app_id, content) in inference_results {
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
                let pos = agent.responses.iter().position(|r| {
                    matches!(r, AgentResponse::InferenceResult { app_id: id, .. } if id == &app_id)
                });
                if let Some(i) = pos {
                    agent.responses.remove(i);
                }
            });
        }
    }
}

async fn handle_host_command(doc_handle: &DocHandle, cmd: HostCommand) {
    match cmd {
        HostCommand::Inference { app_id, content } => {
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
