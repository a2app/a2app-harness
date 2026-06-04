use std::env;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use makepad_widgets::*;

/// Global flag set by the file-watcher thread when splash body changes.
static SPLASH_UPDATED: AtomicBool = AtomicBool::new(false);

app_main!(MakepadWindowApp);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.AgentSplashBase = #(crate::agent_splash::AgentSplash::register_widget(vm))

    mod.widgets.AgentSplash = set_type_default() do mod.widgets.AgentSplashBase{
        width: Fill height: Fit
    }

    startup() do #(MakepadWindowApp::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(980, 760)
                window.title: "Makepad Host"
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 8
                    padding: 14

                    status_line := Label {
                        text: "Waiting for app launch…"
                        draw_text: { color: #xccddff text_style: { font_size: 11 } }
                    }

                    splash_holder := RoundedView {
                        width: Fill height: Fit padding: 12
                        draw_bg.color: #1f232e
                        draw_bg.border_radius: 8.0
                        splash := mod.widgets.AgentSplash{width: Fill height: Fit}
                    }

                    source := TextInput {
                        width: Fill height: 140
                        is_read_only: true is_multiline: true
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct MakepadWindowApp {
    #[live]
    ui: WidgetRef,
    #[rust]
    splash_file: String,
    #[rust]
    status_file: String,
    #[rust]
    last_content: String,
    #[rust]
    marker_written: bool,
    #[rust]
    last_app_id: String,
}

impl MakepadWindowApp {
    fn write_status(&self, status: &str) {
        if !self.status_file.is_empty() {
            let _ = fs::write(&self.status_file, status);
        }
    }

    /// Read the splash body and app ID from the shared file.
    /// The file format is: first line = app_id, rest = splash body.
    fn read_splash_data(&self) -> (String, String) {
        match fs::read_to_string(&self.splash_file) {
            Ok(content) => {
                let content = content.trim().to_string();
                if content.is_empty() {
                    return (String::new(), String::new());
                }
                // First line is the app_id, rest is the splash body
                let mut lines = content.lines();
                let app_id = lines.next().unwrap_or("").to_string();
                let body = lines.collect::<Vec<&str>>().join("\n");
                (app_id, body)
            }
            Err(_) => (String::new(), String::new()),
        }
    }

    fn sync_splash(&mut self, cx: &mut Cx) {
        let (app_id, splash_body) = self.read_splash_data();

        if splash_body.is_empty() && app_id.is_empty() {
            // No app — clear everything
            self.ui.widget(cx, ids!(splash)).set_text(cx, "");
            self.ui.widget(cx, ids!(source)).set_text(cx, "");
            self.ui.label(cx, ids!(status_line))
                .set_text(cx, "Waiting for app launch…");
            self.last_app_id.clear();
            self.last_content.clear();
            return;
        }

        if splash_body != self.last_content || app_id != self.last_app_id {
            eprintln!(
                "[makepad-host] rendering splash for app '{}' ({} chars)",
                app_id,
                splash_body.len()
            );

            self.ui.widget(cx, ids!(splash)).set_text(cx, &splash_body);
            self.ui.widget(cx, ids!(source)).set_text(cx, &splash_body);
            self.ui.label(cx, ids!(status_line))
                .set_text(cx, &format!("App: {}", app_id));

            self.last_app_id = app_id.clone();
            self.last_content = splash_body.clone();
        }
    }

    fn write_window_marker_once(&mut self) {
        if self.marker_written {
            return;
        }
        if let Ok(marker_path) = env::var("MAKEPAD_HOST_WINDOW_MARKER") {
            let _ = fs::write(marker_path, "window-ready\n");
        }
        self.marker_written = true;
    }
}

impl AppMain for MakepadWindowApp {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn after_new_from_script(_vm: &mut ScriptVm, app: &mut Self) {
        app.splash_file = env::var("MAKEPAD_HOST_SPLASH_FILE")
            .unwrap_or_else(|_| "/tmp/makepad-host-splash.txt".to_string());
        app.status_file = env::var("MAKEPAD_HOST_STATUS_FILE")
            .unwrap_or_else(|_| "/tmp/makepad-host-status.txt".to_string());
        app.last_content = String::new();
        app.marker_written = false;
        app.last_app_id = String::new();
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::Startup => {
                eprintln!("[makepad-host] Startup event");
                self.write_window_marker_once();

                // Start the file-watcher thread that polls for splash changes
                let splash_file = self.splash_file.clone();
                thread::spawn(move || {
                    let poll_interval = Duration::from_millis(200);
                    let mut last_mtime: Option<SystemTime> = None;

                    loop {
                        thread::sleep(poll_interval);

                        let current = fs::metadata(&splash_file)
                            .and_then(|m| m.modified())
                            .ok();

                        if current != last_mtime {
                            last_mtime = current;
                            SPLASH_UPDATED.store(true, Ordering::SeqCst);
                        }
                    }
                });

                // Initial sync
                self.sync_splash(cx);
                self.write_status("ready");
            }
            Event::Signal => {
                eprintln!("[makepad-host] Signal received — re-syncing from file");
                self.sync_splash(cx);
            }
            Event::Draw(_) => {
                // Before drawing, check if the watcher thread flagged an update
                if SPLASH_UPDATED.swap(false, Ordering::SeqCst) {
                    self.sync_splash(cx);
                }
            }
            _ => {}
        }
    }
}
