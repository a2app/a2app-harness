use std::cell::Cell;

use makepad_widgets::*;

use crate::SHARED_DOC;

app_main!(MakepadHostApp);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.AgentSplashBase = #(crate::agent_splash::AgentSplash::register_widget(vm))

    mod.widgets.AgentSplash = set_type_default() do mod.widgets.AgentSplashBase{
        width: Fill height: Fit
    }

    startup() do #(MakepadHostApp::script_component(vm)){
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
                        draw_text.color: #xccddff
                        draw_text.text_style.font_size: 11
                    }

                    error_line := Label {
                        text: ""
                        draw_text.color: #xff8888
                        draw_text.text_style.font_size: 10
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

/// Deferred UI changes to be applied on the next Draw event.
/// Set by sync_from_doc (called on Signal), applied by apply_pending_updates (called on Draw).
struct PendingUiUpdate {
    splash_body: Option<String>,
    source_body: Option<String>,
    status: Option<String>,
    error_msg: Option<String>,
    should_exit: bool,
}

#[derive(Script, ScriptHook)]
pub struct MakepadHostApp {
    #[live]
    ui: WidgetRef,
    #[rust]
    last_app_id: String,
    #[rust]
    last_splash_body: String,
    #[rust]
    pending_click: Option<(f64, f64)>, // (x, y) to click on next event cycle
    #[rust]
    pending_type_text: Option<String>, // text to type on next event cycle
    #[rust]
    pending_update: Option<PendingUiUpdate>, // deferred UI changes for next Draw
    #[rust]
    last_error_msg: String,
}

impl MakepadHostApp {
    /// Read the current app state from the shared doc and update the UI.
    fn sync_from_doc(&mut self, _cx: &mut Cx) {
        let doc_handle = match SHARED_DOC.get() {
            Some(h) => h,
            None => return,
        };

        let (app_id, splash_body, should_exit, error_msg, _debug_cmd, _debug_resp) = doc_handle.with_document(|doc| {
            use autosurgeon::hydrate;
            let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
            let id = agent
                .pending_app
                .as_ref()
                .map(|a| a.id.clone())
                .unwrap_or_default();
            let body = agent
                .pending_app
                .as_ref()
                .map(|a| a.splash_body.clone())
                .unwrap_or_default();
            (id, body, agent.should_exit, agent.error_message.clone(), agent.debug_command.clone(), agent.debug_response.clone())
        });

        if should_exit {
            // should_exit — exiting
            std::process::exit(0);
        }

        // Store deferred UI updates (applied on next Draw event)
        let mut update = PendingUiUpdate {
            splash_body: None,
            source_body: None,
            status: None,
            error_msg: None,
            should_exit: false,
        };

        if splash_body.is_empty() && app_id.is_empty() {
            // No app — clear everything
            doc_handle.with_document(|doc| {
                use autosurgeon::{hydrate, reconcile};
                let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
                if agent.error_message.is_some() {
                    let mut agent = agent.clone();
                    agent.error_message = None;
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                }
            });
            update.splash_body = Some(String::new());
            update.source_body = Some(String::new());
            update.status = Some("Waiting for app launch…".to_string());
            self.last_app_id.clear();
            self.last_splash_body.clear();
            self.pending_update = Some(update);
            return;
        }

        // Early return if nothing changed (avoids unnecessary UI updates on idle Signals)
        let error_text = error_msg.as_ref().map(|e| format!("⚠ {}", e)).unwrap_or_default();
        if splash_body == self.last_splash_body
            && app_id == self.last_app_id
            && error_text == self.last_error_msg
        {
            return; // Nothing changed — skip update
        }

        // Something changed — update tracking values
        self.last_error_msg = error_text.clone();
        update.error_msg = Some(error_text);

        if splash_body != self.last_splash_body || app_id != self.last_app_id {
            // Defer splash rendering to the next Draw event
            update.splash_body = Some(splash_body.clone());
            update.source_body = Some(splash_body.clone());
            update.status = Some(format!("App: {}", app_id));

            // Doc status update still happens immediately on Signal
            doc_handle.with_document(|doc| {
                use autosurgeon::{hydrate, reconcile};
                let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
                let needs_update = agent
                    .pending_app
                    .as_ref()
                    .map(|a| a.status == shared::AppStatus::Pending)
                    .unwrap_or(false);
                if needs_update {
                    let mut agent = agent.clone();
                    if let Some(ref mut app) = agent.pending_app {
                        app.status = shared::AppStatus::Launched;
                    }
                    // Only clear error if the body evaluation will succeed;
                    // we can't know yet since we deferred, so keep the error
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                }
            });

            self.last_app_id = app_id;
            self.last_splash_body = splash_body;
        }

        self.pending_update = Some(update);
    }

    /// Process pending debug commands from the shared doc.
    fn process_debug_commands(&mut self, cx: &mut Cx) {
        let doc_handle = match SHARED_DOC.get() {
            Some(h) => h,
            None => return,
        };

        // Read the current debug command
        let cmd = doc_handle.with_document(|doc| {
            use autosurgeon::hydrate;
            let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
            agent.debug_command.clone()
        });

        let Some(cmd) = cmd else {
            // No pending debug command
            return;
        };

        // Process the command
        let result = match cmd.command.as_str() {
            "widget_dump" => {
                // Compact text dump of the widget tree
                let dump = cx.widget_tree().compact_dump(cx);
                Some(dump)
            }
            "widget_snapshot" => {
                // Structured snapshot of all widgets
                let snapshots = cx.widget_tree().snapshot(cx);
                // Convert to JSON manually since WidgetSnapshot doesn't impl Serialize
                let json = snapshot_vec_to_json(&snapshots);
                Some(json)
            }
            "widget_query" => {
                // Query widgets by id/type pattern
                let query = cmd.params.trim();
                let rects = cx.widget_tree().query_rects(cx, query);
                let json = serde_json::to_string(&rects).unwrap_or_else(|e| {
                    format!("[\"serialization failed: {}\"]", e)
                });
                Some(json)
            }
            "click" => {
                // Parse params as JSON: {"widget_id": "..."} or {"x": ..., "y": ...}
                if let Ok(params) = serde_json::from_str::<serde_json::Value>(&cmd.params) {
                    let click_point = if let Some(widget_id) = params.get("widget_id").and_then(|v| v.as_str()) {
                        // Find widget by ID and get its center
                        self.find_widget_center(cx, widget_id)
                    } else if let (Some(x), Some(y)) = (
                        params.get("x").and_then(|v| v.as_f64()),
                        params.get("y").and_then(|v| v.as_f64()),
                    ) {
                        Some((x, y))
                    } else {
                        None
                    };

                    if let Some((x, y)) = click_point {
                        // Store pending click for the next event cycle
                        // (Dispatch during HandleEvent to ensure proper finger/capture state)
                        self.pending_click = Some((x, y));
                        Some(format!("{{\"status\":\"pending\",\"x\":{},\"y\":{}}}", x, y))
                    } else {
                        Some("{\"error\": \"Could not determine click target. Provide widget_id or x,y coordinates.\"}".to_string())
                    }
                } else {
                    // Try parsing as raw coordinates: "x,y"
                    if let Some((x_str, y_str)) = cmd.params.split_once(',') {
                        if let (Ok(x), Ok(y)) = (x_str.trim().parse::<f64>(), y_str.trim().parse::<f64>()) {
                            self.pending_click = Some((x, y));
                            Some(format!("{{\"status\":\"pending\",\"x\":{},\"y\":{}}}", x, y))
                        } else {
                            Some("{\"error\": \"Invalid coordinates. Use format: x,y or JSON {\"widget_id\":\"...\"}\".to_string()}".to_string())
                        }
                    } else {
                        Some("{\"error\": \"Invalid params. Use {\"widget_id\":\"...\"} or {\"x\":...,\"y\":...}\".to_string()}".to_string())
                    }
                }
            }
            "type_text" => {
                // Type text into a focused TextInput widget
                let text = cmd.params.trim().to_string();
                if text.is_empty() {
                    Some("{\"error\": \"No text provided. Provide text to type.\"}".to_string())
                } else {
                    self.pending_type_text = Some(text.clone());
                    Some(format!("{{\"status\":\"pending\",\"text\":\"{}\"}}", text))
                }
            }
            other => {
                Some(format!("{{\"error\": \"Unknown debug command: {}\"}}", other))
            }
        };

        // Write result back to doc (or clear the command on error)
        doc_handle.with_document(|doc| {
            use autosurgeon::{hydrate, reconcile};
            let mut agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
            if let Some(ref json) = result {
                agent.debug_response = Some(json.clone());
            }
            // Clear the command so we don't process it again
            agent.debug_command = None;
            let mut tx = doc.transaction();
            let _ = reconcile(&mut tx, &agent);
            tx.commit();
        });
    }

    /// Find a widget by its ID/name and return the center coordinates.
    fn find_widget_center(&self, cx: &Cx, widget_id: &str) -> Option<(f64, f64)> {
        // Search by path LiveId
        let live_id = LiveId::from_str_lc(widget_id);
        let tree = cx.widget_tree();

        // Try find_within from root
        let root_uid = tree.root_uid();
        if root_uid == WidgetUid(0) {
            return None;
        }

        let widget = tree.find_within(root_uid, &[live_id]);
        if widget.is_empty() {
            return None;
        }

        let area = widget.area();
        if !area.is_valid(cx) {
            return None;
        }

        let rect = area.rect(cx);
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return None;
        }

        Some((rect.pos.x + rect.size.x / 2.0, rect.pos.y + rect.size.y / 2.0))
    }

    /// Apply deferred UI updates that were noted during sync_from_doc.
    /// Called on Draw events before the UI is rendered, ensuring widget
    /// tree mutations happen during the render phase, not during Signal.
    fn apply_pending_updates(&mut self, cx: &mut Cx) {
        let Some(update) = self.pending_update.take() else {
            return;
        };

        if update.should_exit {
            std::process::exit(0);
        }

        if let Some(err) = &update.error_msg {
            self.ui.label(cx, ids!(error_line)).set_text(cx, err);
        }

        if let Some(body) = &update.splash_body {
            self.ui.widget(cx, ids!(splash)).set_text(cx, body);
        }
        if let Some(body) = &update.source_body {
            self.ui.widget(cx, ids!(source)).set_text(cx, body);
        }
        if let Some(status) = &update.status {
            self.ui.label(cx, ids!(status_line)).set_text(cx, status);
        }
    }

    /// Dispatch a pending click by sending MouseDown + MouseUp events.
    /// Events are dispatched directly to the AgentSplash widget, NOT through
    /// the root UI tree, because splash content widgets are orphaned from
    /// the main widget tree (parent = -1) and can't be reached via normal
    /// tree traversal.
    fn dispatch_pending_click(&mut self, cx: &mut Cx) {
        let Some((x, y)) = self.pending_click.take() else {
            return;
        };

        // Get the splash widget to dispatch events directly
        let splash = self.ui.widget(cx, &[id!(splash)]);
        if splash.is_empty() {
            return;
        }

        let abs = dvec2(x, y);
        let modifiers = KeyModifiers::default();
        let time = 0.0;
        let window_id = WindowId(0, 0);

        // MouseDown — dispatch directly to the splash widget
        // The splash's handle_event dispatches to its inner view hierarchy
        let md_event = Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id,
            modifiers,
            handled: Cell::new(Area::Empty),
            time,
        });
        splash.handle_event(cx, &md_event, &mut Scope::empty());

        // MouseUp — dispatch directly to the splash widget
        let mu_event = Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id,
            modifiers,
            time,
        });
        splash.handle_event(cx, &mu_event, &mut Scope::empty());
    }

    /// Dispatch pending type_text by finding a TextInput in the splash
    /// and calling set_text on it directly. Walks ALL widget tree roots
    /// using the compact_dump to find splash TextInput nodes by position.
    fn dispatch_pending_type_text(&mut self, cx: &mut Cx) {
        let Some(text) = self.pending_type_text.take() else {
            return;
        };

        // Get the splash widget to access its child view
        let splash = self.ui.widget(cx, &[id!(splash)]);
        if splash.is_empty() {
            return;
        }

        // Walk the splash's entire view hierarchy via WidgetRef children
        Self::walk_widgets_set_text(splash, cx, &text);
    }

    /// Recursively walk widget children looking for a TextInput and set its text.
    fn walk_widgets_set_text(widget: WidgetRef, cx: &mut Cx, text: &str) -> bool {
        // Check if this widget is a TextInput (not the source code view)
        if widget.borrow::<makepad_widgets::TextInput>().is_some() {
            widget.set_text(cx, text);
            return true;
        }
        // Walk children; stop at first TextInput found
        let mut found = false;
        widget.try_children(&mut |_, child| {
            if !found {
                found = Self::walk_widgets_set_text(child, cx, text);
            }
        });
        found
    }
}

use makepad_widgets::makepad_platform::studio::WidgetSnapshot;

/// Convert a Vec<WidgetSnapshot> to a JSON string (manual since WidgetSnapshot doesn't impl Serialize)
fn snapshot_vec_to_json(snapshots: &[WidgetSnapshot]) -> String {
    let mut items = Vec::new();
    for s in snapshots {
        let obj = serde_json::json!({
            "id": s.id,
            "widget_type": s.widget_type,
            "window_id": s.window_id,
            "window_index": s.window_index,
            "visible": s.visible,
            "enabled": s.enabled,
            "x": s.x,
            "y": s.y,
            "width": s.width,
            "height": s.height,
            "text": s.text,
            "value": s.value,
            "checked": s.checked,
            "selected": s.selected,
        });
        items.push(obj);
    }
    serde_json::to_string_pretty(&items).unwrap_or_else(|e| {
        format!("{{\"error\": \"serialization failed: {e}\"}}")
    })
}

impl AppMain for MakepadHostApp {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn after_new_from_script(_vm: &mut ScriptVm, app: &mut Self) {
        app.last_app_id = String::new();
        app.last_splash_body = String::new();
        app.last_error_msg = String::new();
        app.pending_click = None;
        app.pending_type_text = None;
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // ── Pre-dispatch pending type_text ──────────────────────────
        // Must happen BEFORE self.ui.handle_event() so that AgentSplash
        // sees __pi_type_text during the same event cycle.
        if matches!(event, Event::Signal | Event::Draw(_)) {
            self.dispatch_pending_type_text(cx);
            self.dispatch_pending_click(cx);
        }

        // ── Apply deferred UI updates before rendering ────────────
        // sync_from_doc runs on Signal and stores pending updates in
        // self.pending_update. We apply them on the next Draw (before
        // the UI renders) so that widget tree mutations (splash body
        // eval, set_text) happen during the render phase. We also apply
        // at the end of Signal handling to ensure close/clear operations
        // take effect even if no Draw event follows immediately.
        if matches!(event, Event::Draw(_)) {
            self.apply_pending_updates(cx);
        }

        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::Startup => {
                self.sync_from_doc(cx);
            }
            Event::Signal => {
                self.sync_from_doc(cx);
                self.process_debug_commands(cx);
                // Apply pending updates at end of Signal too, so that
                // close/clear operations render even without a Draw event
                self.apply_pending_updates(cx);
            }
            _ => {}
        }
    }
}
