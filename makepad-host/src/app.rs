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

#[derive(Script, ScriptHook)]
pub struct MakepadHostApp {
    #[live]
    ui: WidgetRef,
    #[rust]
    last_app_id: String,
    #[rust]
    last_splash_body: String,
}

impl MakepadHostApp {
    /// Read the current app state from the shared doc and update the UI.
    fn sync_from_doc(&mut self, cx: &mut Cx) {
        let doc_handle = match SHARED_DOC.get() {
            Some(h) => h,
            None => return,
        };

        let (app_id, splash_body, should_exit, error_msg) = doc_handle.with_document(|doc| {
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
            (id, body, agent.should_exit, agent.error_message.clone())
        });

        if should_exit {
            // should_exit — exiting
            std::process::exit(0);
        }

        // Show error if present
        if let Some(ref err) = error_msg {
            self.ui.label(cx, ids!(error_line)).set_text(cx, &format!("⚠ {}", err));
        } else {
            self.ui.label(cx, ids!(error_line)).set_text(cx, "");
        }

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
            self.ui.widget(cx, ids!(splash)).set_text(cx, "");
            self.ui.widget(cx, ids!(source)).set_text(cx, "");
            self.ui.label(cx, ids!(status_line))
                .set_text(cx, "Waiting for app launch…");
            self.ui.label(cx, ids!(error_line)).set_text(cx, "");
            self.last_app_id.clear();
            self.last_splash_body.clear();
            return;
        }

        if splash_body != self.last_splash_body || app_id != self.last_app_id {
            // rendering splash for app

            self.ui.widget(cx, ids!(splash)).set_text(cx, &splash_body);
            self.ui.widget(cx, ids!(source)).set_text(cx, &splash_body);
            self.ui.label(cx, ids!(status_line))
                .set_text(cx, &format!("App: {}", app_id));

            // Check if an error was reported during rendering (set_text calls eval_body
            // which writes to doc's error_message on failure).
            // If rendering failed, keep the error_message; otherwise clear it.
            let had_error = doc_handle.with_document(|doc| {
                use autosurgeon::hydrate;
                let agent: shared::AgentDoc = hydrate(doc).unwrap_or_default();
                agent.error_message.is_some()
            });

            // Update the doc status from Pending to Launched.
            // Only clear previous error if rendering was successful.
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
                    if !had_error {
                        agent.error_message = None;
                    }
                    let mut tx = doc.transaction();
                    let _ = reconcile(&mut tx, &agent);
                    tx.commit();
                    // app status set to Launched
                }
            });

            self.last_app_id = app_id;
            self.last_splash_body = splash_body;
        }
    }
}

impl AppMain for MakepadHostApp {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn after_new_from_script(_vm: &mut ScriptVm, app: &mut Self) {
        app.last_app_id = String::new();
        app.last_splash_body = String::new();
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::Startup => {
                // Startup event
                self.sync_from_doc(cx);
            }
            Event::Signal => {
                // Doc change signal — re-syncing
                self.sync_from_doc(cx);
            }
            _ => {}
        }
    }
}
