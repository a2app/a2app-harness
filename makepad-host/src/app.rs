use std::env;
use std::fs;

use makepad_widgets::*;

use crate::state::get_host_state;

app_main!(MakepadRootApp);

const MAX_VISIBLE: usize = 8;

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
                    spacing: 6
                    padding: 14

                    status_line := Label {
                        text: "Waiting for CRDT app launch…"
                        draw_text: { color: #xccddff text_style: { font_size: 11 } }
                    }

                    tab_bar := View {
                        width: Fill height: Fit
                        flow: Right spacing: 4
                        padding: 0 0 4 0
                        tab_0: Label { text: "" draw_text: { color: #xbbccdd text_style: { font_size: 11 } } cursor: Hand }
                        tab_1: Label { text: "" draw_text: { color: #xbbccdd text_style: { font_size: 11 } } cursor: Hand }
                        tab_2: Label { text: "" draw_text: { color: #xbbccdd text_style: { font_size: 11 } } cursor: Hand }
                        tab_3: Label { text: "" draw_text: { color: #xbbccdd text_style: { font_size: 11 } } cursor: Hand }
                        tab_4: Label { text: "" draw_text: { color: #xbbccdd text_style: { font_size: 11 } } cursor: Hand }
                        tab_5: Label { text: "" draw_text: { color: #xbbccdd text_style: { font_size: 11 } } cursor: Hand }
                        tab_6: Label { text: "" draw_text: { color: #xbbccdd text_style: { font_size: 11 } } cursor: Hand }
                        tab_7: Label { text: "" draw_text: { color: #xbbccdd text_style: { font_size: 11 } } cursor: Hand }
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

                    other_label := Label {
                        text: "Other apps (0)"
                        draw_text: { color: #xccddff text_style: { font_size: 12 } }
                        margin: 6 0 0 4
                    }
                    card_label := Label {
                        text: ""
                        draw_text: { color: #xddffdd text_style: { font_size: 12 } }
                        margin: 0 0 0 4
                        cursor: Hand
                    }
                }
            }
        }
    }
}

static TAB_IDS: [LiveId; MAX_VISIBLE] = [
    ids!(tab_0)[0], ids!(tab_1)[0], ids!(tab_2)[0], ids!(tab_3)[0],
    ids!(tab_4)[0], ids!(tab_5)[0], ids!(tab_6)[0], ids!(tab_7)[0],
];

static CARD_IDS: [LiveId; MAX_VISIBLE] = [
    ids!(card_0)[0], ids!(card_1)[0], ids!(card_2)[0], ids!(card_3)[0],
    ids!(card_4)[0], ids!(card_5)[0], ids!(card_6)[0], ids!(card_7)[0],
];

fn tab_id(i: usize) -> &'static [LiveId] { &TAB_IDS[i..i+1] }
fn card_id(i: usize) -> &'static [LiveId] { &CARD_IDS[i..i+1] }

#[derive(Script, ScriptHook)]
pub struct MakepadRootApp {
    #[live]
    ui: WidgetRef,
    #[rust]
    last_revision: u64,
    #[rust]
    marker_written: bool,
    #[rust]
    known_apps: Vec<String>,
}

impl MakepadRootApp {
    fn sync_from_host_state(&mut self, cx: &mut Cx) {
        let state = get_host_state();
        let state_lock = state.read().expect("host state poisoned");
        let order = state_lock.app_order.clone();
        let active = state_lock.active_app_id.clone();
        let apps = state_lock.apps.clone();
        self.last_revision = state_lock.revision;
        drop(state_lock);

        self.known_apps = order.clone();

        // Status line
        match active.as_ref() {
            Some(a) => self.ui.label(cx, ids!(status_line))
                .set_text(cx, &format!("Active: {}  |  Total: {}", a, order.len())),
            None if order.is_empty() => self.ui.label(cx, ids!(status_line))
                .set_text(cx, "Waiting for CRDT app launch…"),
            _ => self.ui.label(cx, ids!(status_line))
                .set_text(cx, &format!("Ready · {} app(s) loaded", order.len())),
        }

        // Tabs
        for i in 0..MAX_VISIBLE {
            if let Some(app_id) = order.get(i) {
                let is_active = active.as_deref() == Some(app_id.as_str());
                let prefix = if is_active { "▸ " } else { "" };
                self.ui.label(cx, tab_id(i)).set_text(cx, &format!("{}{}", prefix, app_id));
            } else {
                self.ui.label(cx, tab_id(i)).set_text(cx, "");
            }
        }

        // Active app splash & source
        let active_body = active.as_ref()
            .and_then(|id| apps.get(id))
            .map(|a| a.content.as_str())
            .unwrap_or("");
        self.ui.widget(cx, ids!(splash)).set_text(cx, active_body);
        self.ui.widget(cx, ids!(source)).set_text(cx, active_body);

        // Other apps as plain Labels
        let others: Vec<&String> = order.iter()
            .filter(|id| Some(*id) != active.as_ref())
            .collect();

        // Show non-active apps
        if others.is_empty() {
            self.ui.label(cx, ids!(other_label)).set_text(cx, "Other apps (0)");
        } else {
            let names: Vec<String> = others.iter().map(|s| format!("• {} (click)", s)).collect();
            self.ui.label(cx, ids!(other_label)).set_text(cx, &names.join("\n"));
            eprintln!("[debug] other_label text set, {} app(s)", others.len());
        }
        
        // Set card label with clean app_id for click handling
        if let Some(app_id) = others.get(0) {
            self.ui.label(cx, ids!(card_label)).set_text(cx, &format!("• {}  (click to switch)", app_id));
        } else {
            self.ui.label(cx, ids!(card_label)).set_text(cx, "");
        }
    }

    fn handle_click(&mut self, cx: &mut Cx, abs: Vec2d) {
        let host_state = get_host_state();
        let order = host_state.read().expect("host state poisoned").app_order.clone();
        let count = order.len().min(MAX_VISIBLE);

        // Check tabs
        for i in 0..count {
            let text = self.ui.label(cx, tab_id(i)).text();
            if text.is_empty() { continue; }
            let r = self.ui.label(cx, tab_id(i)).area().rect(cx);
            if r.contains(abs) {
                let app_id = text.trim_start_matches("▸ ").to_owned();
                let mut s = host_state.write().expect("host state poisoned");
                s.set_active_app(&app_id);
                return;
            }
        }

        // Check card label
        let card_text = self.ui.label(cx, ids!(card_label)).text();
        if !card_text.is_empty() {
            let r = self.ui.label(cx, ids!(card_label)).area().rect(cx);
            if r.contains(abs) {
                // Extract app_id: "• counter-1  (click to switch)" -> "counter-1"
                let app_id = card_text
                    .strip_prefix("• ")
                    .and_then(|s| s.split("  (").next())
                    .unwrap_or("")
                    .to_string();
                if !app_id.is_empty() {
                    let mut s = host_state.write().expect("host state poisoned");
                    s.set_active_app(&app_id);
                    return;
                }
            }
        }
    }

    fn write_window_marker_once(&mut self) {
        if self.marker_written { return; }
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
        app.known_apps = Vec::new();
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::Startup => {
                self.write_window_marker_once();
                self.sync_from_host_state(cx);
            }
            Event::Signal => {
                let state = get_host_state();
                let revision = state.read().expect("host state poisoned").revision;
                if revision != self.last_revision {
                    self.sync_from_host_state(cx);
                }
            }
            Event::MouseUp(up) => {
                self.handle_click(cx, up.abs);
            }
            _ => {}
        }
    }
}
