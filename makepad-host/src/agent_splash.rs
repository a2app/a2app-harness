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
    #[rust]
    render_ok: bool,
    /// Tracks the last known text of the __pi_response label
    #[rust]
    last_response: String,
    /// Tracks the last known text of the __pi_data label (data from pi)
    #[rust]
    last_pi_data: String,
    /// Tracks the last known streaming_text for live update display
    #[rust]
    last_streaming_text: String,
}

// The splash body is wrapped in: <PREFIX><body><SUFFIX>
// The parser auto-closes the outer View.
// __pi_response is a hidden label that splash apps can set text on
// to send a response back to the pi extension.
// __pi_data is a hidden label that receives data from the pi extension
// via the shared CRDT doc's pi_response field.
const SPLASH_PREFIX: &str = "use mod.prelude.widgets.*View{height:Fit flow:Down ";
const SPLASH_SUFFIX: &str = "  __ai_text := TextInput{text:\" \" height:34 width:Fill}\n  __pi_response := Label{text:\"\"}\n  __pi_data := Label{text:\" \"}";
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
        // Wrap body with prefix + suffix
        // __pi_response is a hidden label the splash body can set via ui.__pi_response.set_text("...")
        // to send responses back to the pi extension.
        let code = format!("{}{}{}", SPLASH_PREFIX, body, SPLASH_SUFFIX);
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
            // Render an empty View to clear the splash area
            let code = "use mod.prelude.widgets.*View{width:Fill height:Fit}".to_string();
            let script_mod = ScriptMod {
                cargo_manifest_path: String::new(),
                module_path: String::new(),
                file: String::new(),
                line: self.self_id(),
                column: 0,
                code: String::new(),
                values: vec![],
            };
            cx.with_vm(|vm| {
                let value = vm.eval_with_append_source(script_mod, &code, NIL.into());
                if value.is_object() {
                    self.view = View::script_from_value(vm, value);
                }
            });
            return true;
        }

        if self.render_body(cx, &body) {
            true
        } else {
            let _ = self.render_body(cx, SPLASH_ERROR_FALLBACK);
            false
        }
    }


    /// Read streaming_text from the shared doc and live-update __ai_text.
    /// Does NOT clear the doc field — deltas accumulate until the final
    /// pi_response arrives and supersedes it.
    fn sync_streaming_text(&mut self, cx: &mut Cx) {
        let incoming = SHARED_DOC.get().and_then(|handle| {
            handle.with_document(|doc| {
                use autosurgeon::hydrate;
                let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
                agent.streaming_text.clone()
            })
        });

        if let Some(text) = incoming {
            if text != self.last_streaming_text {
                self.last_streaming_text = text.clone();
                let output_widget = self.widget(cx, &[id!(__ai_text)]);
                if !output_widget.is_empty() {
                    output_widget.set_text(cx, &text);
                }
                self.redraw(cx);
            }
        }
    }

    /// Read pi_response from the shared doc and set it on the __pi_data label.
    /// Clears the doc field after reading (one-shot delivery).
    /// Uses a scoped value to avoid nested with_document() calls.
    fn sync_pi_data_to_splash(&mut self, cx: &mut Cx) {
        // Step 1: Read pi_response from doc (outside any widget operations)
        let incoming = SHARED_DOC.get().and_then(|handle| {
            handle.with_document(|doc| {
                use autosurgeon::hydrate;
                let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
                agent.pi_response.clone()
            })
        });

        // Step 2: Update widget if we have new data
        if let Some(data) = incoming {
            if !data.is_empty() && data != self.last_pi_data {
                self.last_pi_data = data.clone();
                let data_widget = self.widget(cx, &[id!(__pi_data)]);
                if !data_widget.is_empty() {
                    data_widget.set_text(cx, &data);
                }
                // Also update the visible __ai_text widget so response auto-displays
                // TextInput uses set_text which triggers re-layout
                let output_widget = self.widget(cx, &[id!(__ai_text)]);
                if !output_widget.is_empty() {
                    output_widget.set_text(cx, &data);
                }
                self.redraw(cx);
                
                // Step 3: Clear the doc field (separate call, no nesting)
                if let Some(handle) = SHARED_DOC.get() {
                    handle.with_document(|doc| {
                        use autosurgeon::{hydrate, reconcile};
                        let mut agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
                        agent.pi_response = None;
                        let mut tx = doc.transaction();
                        let _ = reconcile(&mut tx, &agent);
                        tx.commit();
                    });
                }
            }
        }
    }
}

impl Widget for AgentSplash {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        self.redraw(cx);
        
        // After each event, check if the splash body updated __pi_response.
        // Splash apps call ui.__pi_response.set_text("data") to send
        // data back to the pi extension.
        let response_widget = self.widget(cx, &[id!(__pi_response)]);
        if !response_widget.is_empty() {
            let current = response_widget.text();
            if current != self.last_response && !current.is_empty() {
                let new_response = current.clone();
                self.last_response = current;
                write_doc_field("user_response", new_response.clone());
            }
        }

        // Check if pi sent new data to the splash app.
        // Runs on every event to avoid missing updates when Signal is coalesced.
        self.sync_pi_data_to_splash(cx);

        // Check for streaming text deltas (live sub-agent output)
        self.sync_streaming_text(cx);


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
            self.last_response = String::new(); // reset response tracker
            self.last_pi_data = String::new();
            if !v.is_empty() {
                self.render_ok = self.eval_body(cx);
                if !self.render_ok {
                    report_error("Splash body could not be rendered");
                }
            } else {
                self.render_ok = self.eval_body(cx);
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
                "user_response" => {
                    agent.user_response = Some(value);
                    agent.user_response_version += 1;
                }
                "error_message" => agent.error_message = Some(value),
                "pi_response" => agent.pi_response = Some(value),
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
    // error logged
}

#[allow(dead_code)]
impl AgentSplashRef {
    /// Returns true if the body was rendered successfully, false otherwise.
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
        // response sent
    }
}
