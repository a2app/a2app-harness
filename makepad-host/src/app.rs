use std::env;
use std::fs;

use makepad_widgets::*;

use crate::state::get_host_state;

app_main!(MakepadRootApp);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.AgentSplashBase = #(crate::agent_splash::AgentSplash::register_widget(vm))

    mod.widgets.AgentSplash = set_type_default() do mod.widgets.AgentSplashBase{
        width: Fill height: Fit
    }

    startup() do #(MakepadRootApp::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(980, 760)
                window.title: "Makepad Host"
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 10
                    padding: 14

                    status_line := Label {
                        text: "Waiting for CRDT app launch..."
                    }

                    splash_holder := RoundedView{
                        width: Fill
                        height: Fit
                        padding: 12
                        draw_bg.color: #1f232e
                        draw_bg.border_radius: 8.0
                        splash := mod.widgets.AgentSplash{width: Fill height: Fit}
                    }

                    source_label := Label {text: "Current Splash body"}
                    source := TextInput {
                        width: Fill
                        height: 220
                        is_read_only: true
                        is_multiline: true
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct MakepadRootApp {
    #[live]
    ui: WidgetRef,
    #[rust]
    last_revision: u64,
    #[rust]
    marker_written: bool,
}

impl MakepadRootApp {
    fn sync_from_host_state(&mut self, cx: &mut Cx) {
        let state = get_host_state();
        let state = state.read().expect("host state poisoned");

        let active = state
            .app_order
            .iter()
            .find_map(|id| state.apps.get(id).map(|app| (id.clone(), app.content.clone())));

        match active {
            Some((id, splash_body)) => {
                self.ui
                    .label(cx, ids!(status_line))
                    .set_text(cx, &format!("Running app: {}", id));
                self.ui.widget(cx, ids!(source)).set_text(cx, &splash_body);
                self.ui.widget(cx, ids!(splash)).set_text(cx, &splash_body);
            }
            None => {
                self.ui
                    .label(cx, ids!(status_line))
                    .set_text(cx, "Waiting for CRDT app launch...");
                self.ui.widget(cx, ids!(source)).set_text(cx, "");
                self.ui.widget(cx, ids!(splash)).set_text(cx, "");
            }
        }

        self.last_revision = state.revision;
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

impl AppMain for MakepadRootApp {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn after_new_from_script(_vm: &mut ScriptVm, app: &mut Self) {
        app.last_revision = 0;
        app.marker_written = false;
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());

        if matches!(event, Event::Startup) {
            self.write_window_marker_once();
            self.sync_from_host_state(cx);
            return;
        }

        if matches!(event, Event::Signal) {
            let state = get_host_state();
            let revision = state.read().expect("host state poisoned").revision;
            if revision != self.last_revision {
                self.sync_from_host_state(cx);
            }
        }
    }
}
