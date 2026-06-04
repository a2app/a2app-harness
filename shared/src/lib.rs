use autosurgeon::{Hydrate, Reconcile};

pub const WS_PORT: u16 = 2341;

#[derive(Debug, Clone, Default, Reconcile, Hydrate, PartialEq)]
pub struct AgentDoc {
    /// The single app that the pi extension wants launched.
    /// None = no app running.
    pub pending_app: Option<PendingApp>,
    /// Flag set by the pi extension whenever it makes a change.
    /// The Rust loop resets this and signals the UI.
    pub extension_requests: bool,
    /// Set to true by the extension to request graceful shutdown.
    pub should_exit: bool,
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
