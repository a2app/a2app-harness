use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use shared::App;

use crate::state::{get_host_state, init_host_state, PendingLaunch, SignalToUI};

fn log_preview(content: &str) -> String {
    let mut preview: String = content.chars().take(120).collect();
    preview = preview.replace('\n', "\\n");
    if content.chars().count() > 120 {
        preview.push_str("...");
    }
    preview
}

pub struct MakepadAppHost {
    pub signal: SignalToUI,
}

#[async_trait]
impl App for MakepadAppHost {
    async fn launch_app(&self, id: String, content: String) {
        let state = get_host_state();
        let mut state = state.write().expect("host state poisoned");
        let pending_before = state.pending_launches.len();
        eprintln!(
            "[makepad-host] queue launch {} (pending before: {}, {} chars): {}",
            id,
            pending_before,
            content.chars().count(),
            log_preview(&content)
        );
        state.pending_launches.push(PendingLaunch {
            id: id.clone(),
            content: content.clone(),
        });
        if !state.app_order.iter().any(|existing| existing == &id) {
            state.app_order.push(id.clone());
        }
        state
            .apps
            .entry(id)
            .and_modify(|app| app.content = content.clone())
            .or_insert_with(|| crate::state::AppState::new(content));
        state.bump_revision();
        drop(state);
        self.signal.set();
    }

    async fn handle_inference_response(&self, app_id: String, content: String) {
        let state = get_host_state();
        let mut state = state.write().expect("host state poisoned");
        if let Some(app) = state.apps.get_mut(&app_id) {
            app.last_response = Some(content.clone());
            app.request_in_flight = false;
            if let Some(tx) = app.pending_inference.pop_front() {
                let _ = tx.send(content);
            }
            state.bump_revision();
        }
        drop(state);
        self.signal.set();
    }
}

pub fn run_app() {
    init_host_state(SignalToUI);

    // Placeholder event loop until Makepad wiring is ported.
    loop {
        let _ = get_host_state();
        thread::sleep(Duration::from_millis(16));
    }
}
