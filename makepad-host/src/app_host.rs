use std::env;
use std::thread;
use std::time::Duration;

use makepad_widgets::SignalToUI;

use crate::state::{get_host_state, init_host_state};

/// Initialize the SignalToUI early so doc_agent can post Signal events
/// before the window is fully up.
pub fn init_host_signal() {
    init_host_state(SignalToUI::new());
}

pub fn run_app() {
    // Signal is already initialized by init_host_signal() called from main().
    // The OnceLock ensures we don't double-initialize.
    if env::var("MAKEPAD_HOST_WINDOWED").ok().as_deref() == Some("1") {
        crate::app::app_main();
    } else {
        run_headless_app();
    }
}

fn run_headless_app() {
    // Keep a lightweight fallback mode for non-GUI runs.
    loop {
        let _ = get_host_state();
        thread::sleep(Duration::from_millis(16));
    }
}

