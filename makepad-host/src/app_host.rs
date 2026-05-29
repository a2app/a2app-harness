use std::env;
use std::thread;
use std::time::Duration;

use makepad_widgets::SignalToUI;

use crate::state::{get_host_state, init_host_state};

pub fn run_app() {
    init_host_state(SignalToUI::new());

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

