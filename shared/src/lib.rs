use autosurgeon::{Hydrate, Reconcile};

/// JSON WebSocket port — pi extension ↔ harness (plain JSON, no CRDT)
pub const JSON_WS_PORT: u16 = 2341;

/// samod WebSocket port — harness ↔ makepad-host (CRDT sync between Rust processes)
pub const SAMOD_WS_PORT: u16 = 2342;

#[derive(Debug, Clone, Default, Reconcile, Hydrate, PartialEq)]
pub struct AgentDoc {
    /// The single app that the pi extension wants launched.
    /// None = no app running.
    pub pending_app: Option<PendingApp>,
    /// Flag set by the pi extension whenever it makes a change.
    /// The bridge resets this and signals the Makepad host.
    pub extension_requests: bool,
    /// Set to true by pi to request graceful shutdown.
    pub should_exit: bool,
    /// Optional payload sent by the rendered splash app back to the
    /// pi extension. Written by the Makepad host's AgentSplash widget
    /// (`send_response`), synced to the harness via CRDT, then forwarded
    /// to pi over JSON WS.
    pub user_response: Option<String>,
    /// Error message set by the Makepad host when the splash body
    /// fails to evaluate. The harness bridge loop forwards this to pi
    /// as a `{"type":"error",...}` message.
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Reconcile, Hydrate, PartialEq)]
pub struct PendingApp {
    pub id: String,
    pub splash_body: String,
    pub status: AppStatus,
}

#[derive(Debug, Clone, Reconcile, Hydrate, PartialEq)]
pub enum AppStatus {
    Pending,
    Launched,
}
