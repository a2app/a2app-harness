use makepad_widgets::*;

use crate::SHARED_DOC;
use crate::STREAMING_RX;

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
    #[rust]
    last_response: String,
    #[rust]
    last_pi_data: String,
    #[rust]
    last_streaming_text: String,
    #[live(true)]
    is_root: bool,
}

const SPLASH_PREFIX: &str = "use mod.prelude.widgets.*View{width:Fill height:Fit flow:Down ";
const SPLASH_SUFFIX: &str = "  __run_splash := mod.widgets.AgentSplash{width:Fill height:Fit is_root:false}
  __pi_status := Label{text:\" \" height:Fit width:Fill}\n  __ai_text := Label{text:\" \" height:Fit width:Fill}\n  __pi_response := Label{text:\"\" visible:false}\n  __pi_data := Label{text:\" \" visible:false}";

impl AgentSplash {
    fn self_id(&self) -> usize {
        self as *const Self as usize
    }

    fn render_body(&mut self, cx: &mut Cx, body: &str) -> bool {
        let self_id = self.self_id();
        let widget_uid = self.widget_uid();
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
        self.render_body(cx, &body)
    }

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
                let previous = self.last_streaming_text.clone();
                self.last_streaming_text = text.clone();

                let output_widget = self.widget(cx, &[id!(__ai_text)]);
                if !output_widget.is_empty() {
                    output_widget.set_text(cx, &text);
                }

                // Update status: "⚙ Generating..." when first code marker appears,
                // "✅ Done" when a complete code block (with closing ```) is extracted.
                let has_any_marker = text.contains("```runsplash") || text.contains("```splash") || text.contains("\n```\n");
                let had_any_marker = previous.contains("```runsplash") || previous.contains("```splash") || previous.contains("\n```\n");
                let first_code_block = has_any_marker && !had_any_marker;
                // Check if the text after any marker has a closing ```
                let has_closing_marker = (text.contains("```runsplash") && text[text.find("```runsplash").unwrap() + 12..].contains("```"))
                    || (text.contains("```splash") && text[text.find("```splash").unwrap() + 10..].contains("```"))
                    || (text.contains("\n```\n") && text[text.rfind("\n```\n").unwrap() + 5..].trim().contains("```"));
                let status_widget = self.widget(cx, &[id!(__pi_status)]);
                if !status_widget.is_empty() {
                    if has_closing_marker {
                        status_widget.set_text(cx, "✅ Done");
                    } else if first_code_block {
                        status_widget.set_text(cx, "⚙ Generating...");
                    }
                }

                // Update log widget for all streaming (non-runsplash chat)
                let log_widget = self.widget(cx, &[id!(log)]);
                if !log_widget.is_empty() {
                    let current = log_widget.text();
                    let current = if current == " " { "" } else { current.as_str() };
                    if first_code_block {
                        // First code block: show generating status
                        if current.is_empty() {
                            log_widget.set_text(cx, "⚙ Generating...");
                        } else if let Some(ai_marker) = current.rfind("\n🤖 ") {
                            log_widget.set_text(cx, &format!("{}\n⚙ Generating...", &current[..ai_marker]));
                        } else {
                            log_widget.set_text(cx, &format!("{}\n⚙ Generating...", current));
                        }
                    } else if !has_any_marker {
                        // Normal chat: update with 🤖 prefix (replace last line)
                        if previous.is_empty() {
                            // First streaming delta: append a new line
                            let new_text = if current.is_empty() {
                                format!("🤖 {}", text)
                            } else {
                                format!("{}\n🤖 {}", current, text)
                            };
                            log_widget.set_text(cx, &new_text);
                        } else {
                            // Subsequent deltas: replace the last "🤖 " line
                            if let Some(ai_marker) = current.rfind("\n🤖 ") {
                                let prefix = &current[..ai_marker];
                                log_widget.set_text(cx, &format!("{}\n🤖 {}", prefix, text));
                            } else if current.starts_with("🤖 ") {
                                log_widget.set_text(cx, &format!("🤖 {}", text));
                            }
                        }
                    }
                }

                // Try to render generated Splash code during streaming so the UI
                // builds up progressively as the AI generates it.
                // Check for these marker patterns (in order of preference):
                //   ```runsplash ... ```
                //   ```splash ... ```
                //   ``` ... ```  (plain backtick block)
                // set_text has built-in error recovery: if eval_body fails, it
                // restores the previous valid body, so failed partial parses
                // are silently ignored and the last valid UI persists.
                let runsplash_marker_start = "```runsplash";
                let splash_marker_start = "```splash";
                let plain_marker_start = "```";
                let runsplash_marker_end = "```";
                // Try to extract code from marker types (```runsplash, ```splash, ```)
                let mut rendered_code: Option<String> = None;
                let marker_types = [runsplash_marker_start, splash_marker_start, plain_marker_start];
                for &marker in &marker_types {
                    if let Some(block_start) = text.find(marker) {
                        let abs_start = block_start + marker.len();
                        // Check if this is actually a plain ``` that overlaps with ```splash
                        if marker == plain_marker_start {
                            let rest_before = &text[..block_start];
                            if rest_before.contains(splash_marker_start) || rest_before.contains(runsplash_marker_start) {
                                continue;
                            }
                        }
                        if let Some(block_end) = text[abs_start..].find(runsplash_marker_end) {
                            let extracted = text[abs_start..abs_start + block_end].trim();
                            if !extracted.is_empty() {
                                rendered_code = Some(extracted.to_string());
                                break;
                            }
                        } else {
                            // No closing ``` yet — streaming in progress
                            let rest = &text[abs_start..];
                            if rest.len() > 15 {
                                let extracted = rest.trim();
                                if !extracted.is_empty() {
                                    rendered_code = Some(extracted.to_string());
                                }
                            }
                            break;
                        }
                    }
                }
                // If no marker found and text looks like Splash DSL (starts with let or RoundedView),
                // try rendering the text directly as Splash code.
                if rendered_code.is_none() {
                    let trimmed = text.trim();
                    if trimmed.len() > 20 && (trimmed.starts_with("let ") || trimmed.starts_with("fn ") || trimmed.starts_with("RoundedView")) {
                        rendered_code = Some(trimmed.to_string());
                    }
                }

                if let Some(runsplash_code) = rendered_code {
                    let run_splash = self.widget(cx, &[id!(__run_splash)]);
                    if !run_splash.is_empty() {
                        let current_body = run_splash.text();
                        if runsplash_code != current_body {
                            run_splash.set_text(cx, &runsplash_code);
                            // Force full tree re-layout by marking root dirty.
                            // The nested AgentSplash grows when set_text replaces
                            // its inner View; the parent must recalculate layout
                            // or the stale cached dimensions produce NaN.
                            cx.widget_tree_mark_dirty(self.widget_uid());
                        }
                    }
                }
            }
        }
    }

    fn sync_pi_data_to_splash(&mut self, cx: &mut Cx) {
        let incoming = SHARED_DOC.get().and_then(|handle| {
            handle.with_document(|doc| {
                use autosurgeon::hydrate;
                let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
                agent.pi_response.clone()
            })
        });

        if let Some(data) = incoming {
            if !data.is_empty() && data != self.last_pi_data {
                self.last_pi_data = data.clone();

                let runsplash_marker_start = "```runsplash";
                let runsplash_marker_end = "```";
                let mut last_extracted: Option<String> = None;
                if let Some(start) = data.find(runsplash_marker_start) {
                    let code_start = start + runsplash_marker_start.len();
                    if let Some(end) = data[code_start..].find(runsplash_marker_end) {
                        let extracted = data[code_start..code_start + end].trim();
                        if !extracted.is_empty() {
                            last_extracted = Some(extracted.to_string());
                        }
                    }
                }

                if let Some(runsplash_code) = last_extracted {
                    let run_splash = self.widget(cx, &[id!(__run_splash)]);
                    if !run_splash.is_empty() {
                        let current_body = run_splash.text();
                        if runsplash_code != current_body {
                            run_splash.set_text(cx, &runsplash_code);
                        }
                    }
                    // Show completion status for runsplash generation
                    let status_widget = self.widget(cx, &[id!(__pi_status)]);
                    if !status_widget.is_empty() {
                        status_widget.set_text(cx, "✅ Done");
                        cx.widget_tree_mark_dirty(self.widget_uid());
                        self.redraw(cx);
                    }
                }

                let data_widget = self.widget(cx, &[id!(__pi_data)]);
                if !data_widget.is_empty() {
                    data_widget.set_text(cx, &data);
                }
                let output_widget = self.widget(cx, &[id!(__ai_text)]);
                if !output_widget.is_empty() {
                    output_widget.set_text(cx, &data);
                }
                let log_widget = self.widget(cx, &[id!(log)]);
                if !log_widget.is_empty() {
                    let current = log_widget.text();
                    let current = if current == " " { "" } else { current.as_str() };
                    if current.contains("⚙") {
                        let new_text = if let Some(ai_marker) = current.rfind("\n⚙ ") {
                            format!("{}\n✅ Done", &current[..ai_marker])
                        } else if current.starts_with("⚙") || current.is_empty() {
                            "✅ Done".to_string()
                        } else {
                            format!("{}\n✅ Done", current)
                        };
                        log_widget.set_text(cx, &new_text);
                    } else {
                        let new_text = if let Some(ai_marker) = current.rfind("\n🤖 ") {
                            format!("{}🤖 {}\n", &current[..ai_marker + 1], data)
                        } else if current.starts_with("🤖 ") || current.is_empty() {
                            format!("🤖 {}\n", data)
                        } else {
                            format!("{}\n🤖 {}\n", current, data)
                        };
                        log_widget.set_text(cx, &new_text);
                    }
                }
                self.redraw(cx);

                // Reset streaming tracker so the next prompt starts fresh
                self.last_streaming_text = String::new();

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

        let response_widget = self.widget(cx, &[id!(__pi_response)]);
        if !response_widget.is_empty() {
            let current = response_widget.text();
            if current != self.last_response && !current.is_empty() {
                let new_response = current.clone();
                self.last_response = current;
                write_doc_field("user_response", new_response.clone());
            }
        }

        if let Some(rx) = STREAMING_RX.get() {
            if let Ok(mut rx) = rx.lock() {
                while let Ok(delta) = rx.try_recv() {
                    if delta != self.last_streaming_text {
                        self.last_streaming_text = delta.clone();
                        let output_widget = self.widget(cx, &[id!(__ai_text)]);
                        if !output_widget.is_empty() {
                            output_widget.set_text(cx, &delta);
                        }
                        let log_widget = self.widget(cx, &[id!(log)]);
                        if !log_widget.is_empty() && !delta.contains("```runsplash") {
                            let current = log_widget.text();
                            let current = if current == " " { "" } else { current.as_str() };
                            if let Some(ai_marker) = current.rfind("\n🤖 ") {
                                log_widget.set_text(cx, &format!("{}🤖 {}", &current[..ai_marker + 1], delta));
                            } else if current.starts_with("🤖 ") || current.is_empty() {
                                log_widget.set_text(cx, &format!("🤖 {}", delta));
                            } else {
                                log_widget.set_text(cx, &format!("{}\n🤖 {}", current, delta));
                            }
                        }
                        self.redraw(cx);
                    }
                }
            }
        }

        // Sync CRDT state on Signal events (when data changes).
        // Also fallback on Draw events while streaming is active, since
        // rapid CRDT writes can be coalesced into a single Signal.
        if matches!(event, Event::Signal) && self.is_root {
            self.sync_streaming_text(cx);
            self.sync_pi_data_to_splash(cx);
        } else if !self.last_streaming_text.is_empty() && matches!(event, Event::Draw(_)) && self.is_root {
            self.sync_streaming_text(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn text(&self) -> String {
        self.body.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.body.as_ref() != v {
            let prev_body = self.body.as_ref().to_string();
            self.body.set(v);
            self.last_response = String::new();
            self.last_pi_data = String::new();
            if !v.is_empty() {
                self.render_ok = self.eval_body(cx);
                if !self.render_ok {
                    self.body.set(&prev_body);
                    self.eval_body(cx);
                    self.render_ok = true;
                }
            } else {
                self.render_ok = self.eval_body(cx);
            }
            self.redraw(cx);
        }
    }
}

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

#[allow(dead_code)]
impl AgentSplashRef {
    pub fn set_text(&self, cx: &mut Cx, v: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, v);
        }
    }

    pub fn send_response(&self, _cx: &mut Cx, data: &str) {
        write_doc_field("user_response", data.to_string());
    }
}
