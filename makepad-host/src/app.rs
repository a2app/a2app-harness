use std::env;
use std::fs;

use makepad_widgets::*;

use crate::state::{get_host_state, send_command, AppState, ChatMessage, HostCommand};

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

                    content_area := View {
                        width: Fill height: Fit
                        flow: Down spacing: 6

                        splash_holder := RoundedView {
                            width: Fill height: Fit padding: 12
                            draw_bg.color: #1f232e
                            draw_bg.border_radius: 8.0
                            splash := mod.widgets.AgentSplash{width: Fill height: Fit}
                        }

                        chat_input_row := RoundedView {
                            width: Fill height: Fit
                            flow: Right spacing: 8
                            padding: Inset{top: 6 bottom: 6 left: 12 right: 12}
                            visible: false
                            draw_bg.color: #x262a36
                            draw_bg.border_radius: 8.0
                            chat_mode := Label {
                                text: "[mode: sub]"
                                draw_text: { color: #xaadd99 text_style: { font_size: 11 } }
                                cursor: Hand
                            }
                            chat_input := TextInput {
                                width: Fill height: 34
                                empty_text: "Type a message..."
                            }
                            chat_send := Button {
                                text: "Send" width: 80 height: 34
                            }
                        }
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

/// Generate a Splash DSL body that renders chat messages as styled label widgets.
/// This is used for `__chat__` apps — the host generates the body dynamically.
fn generate_chat_splash_body(messages: &[ChatMessage], app_state: Option<&AppState>) -> String {
    let has_pending = app_state
        .and_then(|a| a.last_response.as_ref())
        .filter(|resp| !messages.iter().any(|m| m.role == "assistant" && m.content == **resp))
        .is_some();

    let mut body = r#"RoundedView{
    width: Fill height: Fit
    flow: Down spacing: 6
    padding: 12
    new_batch: true
    draw_bg.color: #x1a1a2e
    draw_bg.border_radius: 6.0
"#.to_string();

    if messages.is_empty() && !has_pending {
        body.push_str(r#"    Label{
        text: "Start a conversation! Type a message below."
        draw_text: { color: #x8899aa text_style: { font_size: 11 } }
    }"#);
    } else {
        let mut all_msgs: Vec<(String, String)> = messages
            .iter()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect();

        // If there's a pending response not yet in chat_messages, show it
        if let Some(pending) = app_state
            .and_then(|a| a.last_response.as_ref())
            .filter(|resp| !messages.iter().any(|m| m.role == "assistant" && m.content == **resp))
        {
            all_msgs.push(("assistant".to_string(), pending.clone()));
        }

        for (role, content) in &all_msgs {
            let prefix = if role == "user" { "You" } else { "AI" };
            let color = if role == "user" { "#x88ddff" } else { "#xddd" };
            // Escape double quotes and backslashes in content for Splash DSL
            let escaped = content
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n");
            body.push_str(&format!(
                r#"    RoundedView{{
        width: Fill height: Fit
        flow: Down spacing: 2
        padding: Inset{{top: 6 bottom: 6 left: 10 right: 10}}
        new_batch: true
        draw_bg.color: #x2a2a3e
        draw_bg.border_radius: 6.0
        Label{{
            text: "{}:"
            draw_text: {{ color: {} text_style: {{ font_size: 10 }} }}
        }}
        Label{{
            text: "{}"
            draw_text: {{ color: #xeee text_style: {{ font_size: 11 }} }}
        }}
    }}"#,
                prefix, color, escaped
            ));
        }
    }

    body.push_str("}");
    body
}

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
        let chat_msgs = active.as_ref()
            .and_then(|id| state_lock.chat_messages.get(id))
            .cloned()
            .unwrap_or_default();
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
        let is_chat = active_body == "__chat__";

        // Show/hide splash holder and chat input row
        self.ui.view(cx, ids!(splash_holder)).set_visible(cx, true);
        self.ui.view(cx, ids!(chat_input_row)).set_visible(cx, is_chat);

        if is_chat {
            // Show current inference mode
            if let Some(app_id) = active.as_ref() {
                if let Some(app_state) = apps.get(app_id) {
                    let mode = &app_state.inference_mode;
                    self.ui.label(cx, ids!(chat_mode)).set_text(cx, &format!("[mode: {}]", mode));
                }
            }
            // Build a dynamic Splash body that renders all chat messages
            let splash_body = generate_chat_splash_body(&chat_msgs, active.as_ref().and_then(|id| apps.get(id)));
            self.ui.widget(cx, ids!(splash)).set_text(cx, &splash_body);
            self.ui.widget(cx, ids!(source)).set_text(cx, &splash_body);
        } else {
            self.ui.widget(cx, ids!(splash)).set_text(cx, active_body);
            self.ui.widget(cx, ids!(source)).set_text(cx, active_body);
        }

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

        // Check chat mode toggle (only when chat input row is visible)
        let is_chat_visible = self.ui.view(cx, ids!(chat_input_row)).visible();
        if is_chat_visible {
            let mode_rect = self.ui.label(cx, ids!(chat_mode)).area().rect(cx);
            if mode_rect.contains(abs) {
                let host_state = get_host_state();
                let mut state = host_state.write().expect("host state poisoned");
                if let Some(ref aid) = state.active_app_id.clone() {
                    if let Some(app) = state.apps.get_mut(aid) {
                        app.inference_mode = if app.inference_mode == "sub" {
                            "full".to_string()
                        } else {
                            "sub".to_string()
                        };
                        state.bump_revision();
                    }
                }
                return;
            }

            // Check chat send button
            let send_rect = self.ui.button(cx, ids!(chat_send)).area().rect(cx);
            if send_rect.contains(abs) {
                let input_text = self.ui.text_input(cx, ids!(chat_input)).text();
                let trimmed = input_text.trim().to_string();
                if !trimmed.is_empty() {
                    let (app_id, mode) = {
                        let state = get_host_state();
                        let state_lock = state.read().expect("host state poisoned");
                        let aid = state_lock.active_app_id.clone();
                        let m = aid.as_ref()
                            .and_then(|id| state_lock.apps.get(id))
                            .map(|a| a.inference_mode.clone())
                            .unwrap_or_else(|| "sub".to_string());
                        (aid, m)
                    };
                    if let Some(ref aid) = app_id {
                        // Add user message to chat history and signal the UI
                        {
                            let host_state_arc = get_host_state();
                            let mut state = host_state_arc.write().expect("host state poisoned");
                            state
                                .chat_messages
                                .entry(aid.clone())
                                .or_default()
                                .push(ChatMessage {
                                    role: "user".to_string(),
                                    content: trimmed.clone(),
                                });
                            state.bump_revision();
                            if let Some(ref sig) = state.signal {
                                sig.set();
                            }
                        }
                        // Send inference request with mode
                        eprintln!("[app] sending Inference command for '{}' mode={}", aid, mode);
                        let result = send_command(HostCommand::Inference {
                            app_id: aid.clone(),
                            content: trimmed,
                            mode: mode.clone(),
                        });
                        if let Err(e) = result {
                            eprintln!("[app] send_command FAILED: {}", e);
                        } else {
                            eprintln!("[app] send_command succeeded");
                        }
                        // Clear input
                        self.ui.text_input(cx, ids!(chat_input)).set_text(cx, "");
                    }
                }
                return;
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

/// Helper: if the active app is a chat app with a pending last_response, absorb it into chat_messages.
/// Returns true if something was absorbed.
fn app_is_chat_and_has_response(state_lock: &mut crate::state::HostState) -> bool {
    let active_id = match state_lock.active_app_id.clone() {
        Some(id) => id,
        None => return false,
    };
    let app = match state_lock.apps.get_mut(&active_id) {
        Some(app) => app,
        None => return false,
    };
    if app.content != "__chat__" {
        return false;
    }
    let resp = match app.last_response.take() {
        Some(r) => r,
        None => return false,
    };
    eprintln!("[app] absorbing response for '{}': {} chars", active_id, resp.len());
    let msgs = state_lock.chat_messages.entry(active_id).or_default();
    if msgs.iter().any(|m| m.role == "assistant" && m.content == resp) {
        eprintln!("[app] duplicate response, skipping");
        return false;
    }
    msgs.push(ChatMessage {
        role: "assistant".to_string(),
        content: resp,
    });
    true
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
                let mut should_sync = false;
                {
                    let state = get_host_state();
                    let mut state_lock = state.write().expect("host state poisoned");
                    let revision = state_lock.revision;
                    eprintln!("[app] Signal: revision={}, last_revision={}, active={:?}",
                        revision, self.last_revision, state_lock.active_app_id);
                    if revision != self.last_revision {
                        // If there's a pending inference response for a chat app, absorb it
                        if app_is_chat_and_has_response(&mut state_lock) {
                            state_lock.bump_revision();
                            eprintln!("[app] response absorbed, chat_msgs now {}",
                                state_lock.chat_messages.get(
                                    state_lock.active_app_id.as_deref().unwrap_or("")
                                ).map_or(0, |v| v.len()));
                        }
                        let final_revision = state_lock.revision;
                        if final_revision != self.last_revision {
                            self.last_revision = final_revision;
                            should_sync = true;
                        }
                    }
                }
                if should_sync {
                    eprintln!("[app] calling sync_from_host_state");
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
