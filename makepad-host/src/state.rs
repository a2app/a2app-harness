use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock, RwLock};

use makepad_widgets::SignalToUI;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub enum HostCommand {
    Inference { app_id: String, content: String },
    CloseApp(String),
}

pub struct PendingLaunch {
    pub id: String,
    pub content: String,
}

pub struct AppState {
    pub content: String,
    pub last_request: Option<String>,
    pub last_response: Option<String>,
    pub request_in_flight: bool,
    pub pending_inference: VecDeque<oneshot::Sender<String>>,
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            last_request: self.last_request.clone(),
            last_response: self.last_response.clone(),
            request_in_flight: self.request_in_flight,
            // Senders cannot be cloned; new clones start with empty queue.
            pending_inference: VecDeque::new(),
        }
    }
}

impl AppState {
    pub fn new(content: String) -> Self {
        Self {
            content,
            last_request: None,
            last_response: None,
            request_in_flight: false,
            pending_inference: VecDeque::new(),
        }
    }
}

pub struct HostState {
    pub revision: u64,
    pub pending_launches: Vec<PendingLaunch>,
    pub app_order: Vec<String>,
    pub apps: HashMap<String, AppState>,
    pub active_app_id: Option<String>,
    pub signal: Option<SignalToUI>,
}

impl HostState {
    pub fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn ensure_active_app(&mut self) {
        if let Some(ref id) = self.active_app_id {
            if self.apps.contains_key(id) {
                return;
            }
        }
        self.active_app_id = self.app_order.first().cloned();
    }

    pub fn set_active_app(&mut self, id: &str) {
        if self.apps.contains_key(id) {
            self.active_app_id = Some(id.to_string());
            self.bump_revision();
            if let Some(ref sig) = self.signal {
                sig.set();
            }
        }
    }
}

pub static HOST_STATE: OnceLock<Arc<RwLock<HostState>>> = OnceLock::new();
pub static COMMAND_TX: OnceLock<mpsc::UnboundedSender<HostCommand>> = OnceLock::new();

pub fn init_host_state(signal: SignalToUI) {
    let state = HOST_STATE
        .get_or_init(|| {
            Arc::new(RwLock::new(HostState {
                revision: 0,
                pending_launches: Vec::new(),
                app_order: Vec::new(),
                apps: HashMap::new(),
                active_app_id: None,
                signal: None,
            }))
        })
        .clone();
    if let Ok(mut s) = state.write() {
        s.signal = Some(signal);
    }
}

pub fn get_host_state() -> Arc<RwLock<HostState>> {
    HOST_STATE
        .get_or_init(|| {
            Arc::new(RwLock::new(HostState {
                revision: 0,
                pending_launches: Vec::new(),
                app_order: Vec::new(),
                apps: HashMap::new(),
                active_app_id: None,
                signal: None,
            }))
        })
        .clone()
}

pub fn get_signal() -> Option<SignalToUI> {
    get_host_state().read().ok().and_then(|s| s.signal.clone())
}

pub fn send_command(command: HostCommand) -> Result<(), String> {
    let tx = COMMAND_TX
        .get()
        .ok_or_else(|| "Makepad backend is not ready yet.".to_string())?;

    tx.send(command)
        .map_err(|_| "Makepad backend command channel is closed.".to_string())
}
