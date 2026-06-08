use makepad_widgets::*;

use crate::SHARED_DOC;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AgentSplashBase = #(AgentSplash::register_widget(vm))

    mod.widgets.AgentSplash = set_type_default() do mod.widgets.AgentSplashBase{
        width: Fill height: Fit
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct AgentSplash {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    pub view: View,
    #[live]
    body: ArcStringMut,
}

const SPLASH_PREFIX: &str = "use mod.prelude.widgets.*\n";
const SPLASH_ERROR_FALLBACK: &str = r#"RoundedView{
    width: Fill height: Fit
    flow: Down spacing: 8
    padding: 12
    new_batch: true
    draw_bg.color: #x2a1f24
    draw_bg.border_radius: 8.0
    Label{text: \"Splash app could not be rendered\" draw_text.color: #fff draw_text.text_style.font_size: 13}
    Label{text: \"The generated Splash body was rejected by Makepad.\" draw_text.color: #e3c8ce draw_text.text_style.font_size: 10}
}"#;

impl AgentSplash {
    fn self_id(&self) -> usize {
        self as *const Self as usize
    }

    fn render_body(&mut self, cx: &mut Cx, body: &str) -> bool {
        let self_id = self.self_id();
        let widget_uid = self.widget_uid();
        let code = format!("{}{}", SPLASH_PREFIX, body);
        let script_mod = ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: String::new(),
            line: self_id,
            column: 0,
            code: String::new(),
            values: vec![],
        };

        cx.with_vm(|vm| {
            let value = vm.eval_with_append_source(script_mod, &code, NIL.into());
            // Makepad's parser is lenient; check both error flags and result type
            if value.is_err() || value.is_nil() || !value.is_object() {
                return false;
            }
            self.view = View::script_from_value(vm, value);
            vm.cx_mut().widget_tree_mark_dirty(widget_uid);
            true
        })
    }

    fn eval_body(&mut self, cx: &mut Cx) -> bool {
        let body = self.body.as_ref().to_string();
        if body.is_empty() {
            return true;
        }

        if self.render_body(cx, &body) {
            true
        } else {
            let _ = self.render_body(cx, SPLASH_ERROR_FALLBACK);
            false
        }
    }
}

impl Widget for AgentSplash {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn text(&self) -> String {
        self.body.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.body.as_ref() != v {
            self.body.set(v);
            if !v.is_empty() {
                let ok = self.eval_body(cx);
                if !ok {
                    report_error("Splash body could not be rendered");
                }
            }
            self.redraw(cx);
        }
    }
}

/// Write a field on the shared AgentDoc.
fn write_doc_field(field: &str, value: String) {
    if let Some(handle) = SHARED_DOC.get() {
        handle.with_document(|doc| {
            use autosurgeon::{hydrate, reconcile};
            let mut agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
            match field {
                "user_response" => agent.user_response = Some(value),
                "error_message" => agent.error_message = Some(value),
                _ => {}
            }
            let mut tx = doc.transaction();
            let _ = reconcile(&mut tx, &agent);
            tx.commit();
        });
    }
}

/// Report an error to the pi extension by writing to the doc's `error_message` field.
fn report_error(message: &str) {
    write_doc_field("error_message", message.to_string());
    eprintln!("[splash] error: {}", message);
}

#[allow(dead_code)]
impl AgentSplashRef {
    pub fn set_text(&self, cx: &mut Cx, v: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, v);
        }
    }

    /// Bridge for splash apps to send a response back to the pi extension.
    /// Writes the response into the shared CRDT document's `user_response` field,
    /// which the harness sees via CRDT sync and forwards to pi over JSON WS.
    pub fn send_response(&self, _cx: &mut Cx, data: &str) {
        write_doc_field("user_response", data.to_string());
        eprintln!("[splash] send_response: {}", data);
    }
}
