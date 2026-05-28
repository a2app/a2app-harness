use autosurgeon::{Hydrate, Reconcile};
use async_trait::async_trait;
use std::collections::HashMap;

pub const WS_PORT: u16 = 2341;
pub const DOC_ID_PORT: u16 = 2348;

#[derive(Debug, Default, Clone, Reconcile, Hydrate, PartialEq)]
pub struct AgentDoc {
    pub requests: Vec<AgentRequest>,
    pub responses: Vec<AgentResponse>,
    pub mini_apps: HashMap<String, MiniApp>,
    pub conversation_history: Vec<ConversationFragment>,
    pub stored_values: HashMap<String, StoredValue>,
    pub text_documents: HashMap<String, String>,
    pub active_document: Option<String>,
    pub should_exit: bool,
    pub active_model: Option<String>,
}

#[derive(Debug, Clone, Reconcile, Hydrate, PartialEq)]
pub enum AgentRequest {
    LaunchApp { id: String, splash_body: String },
    CloseApp { id: String },
    Inference { content: String, app_id: String },
}

#[derive(Debug, Clone, Reconcile, Hydrate, PartialEq)]
pub enum AgentResponse {
    AppLaunched { id: String },
    AppClosed { id: String },
    InferenceResult { app_id: String, content: String },
    Chat(String),
}

#[derive(Debug, Clone, Reconcile, Hydrate, PartialEq, Default)]
pub struct MiniApp {
    pub splash_body: String,
    pub state: HashMap<String, String>,
}

#[derive(Debug, Clone, Reconcile, Hydrate, PartialEq)]
pub enum ConversationFragment {
    User(String),
    Assistant(String),
}

#[derive(Debug, Clone, Reconcile, Hydrate, PartialEq, Default)]
pub struct StoredValue {
    pub value: String,
    pub description: String,
}

#[async_trait]
pub trait App: Send + Sync {
    async fn launch_app(&self, id: String, content: String);
    async fn handle_inference_response(&self, app_id: String, content: String);
}
