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
    /// Monotonically increasing version counter for user_response.
    /// Incremented by makepad-host on each write so the bridge loop
    /// can detect changes even when the value is the same.
    pub user_response_version: u64,
    /// Error message set by the Makepad host when the splash body
    /// fails to evaluate. The harness bridge loop forwards this to pi
    /// as a `{"type":"error",...}` message.
    pub error_message: Option<String>,
    /// Debug command from pi to the makepad-host.
    /// Set by the harness when pi sends a debug request,
    /// cleared by the makepad-host after processing.
    pub debug_command: Option<DebugCommand>,
    /// Debug response from the makepad-host back to pi.
    /// Set by the makepad-host after processing a debug command,
    /// cleared by the harness after forwarding to pi.
    pub debug_response: Option<String>,
    /// Data sent from pi to the splash app.
    /// Written by the harness when pi sends a send_pi_response message,
    /// read and cleared by the makepad-host's AgentSplash.
    pub pi_response: Option<String>,
    /// Streaming text accumulated from sub-agent deltas.
    /// Written by the harness as deltas arrive from the pi extension.
    /// Read by the makepad-host's AgentSplash for live display.
    /// Cleared when the final pi_response is written.
    pub streaming_text: Option<String>,
    /// When the makepad-host panics during handle_event, the panic backtrace
    /// is captured and written here so the harness + pi can read it.
    pub panic_backtrace: Option<String>,
}

#[derive(Debug, Clone, Reconcile, Hydrate, PartialEq)]
pub struct DebugCommand {
    /// The type of debug command: "widget_dump", "widget_snapshot", "widget_query", "click", "type_text"
    pub command: String,
    /// JSON-encoded parameters (varies by command)
    pub params: String,
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
