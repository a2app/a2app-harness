use std::collections::HashMap;

use crate::doc::{AgentRequest, AgentResponse, MiniApp};
use crate::repo::DocHandle;

pub async fn run(doc_handle: DocHandle) {
    loop {
        if doc_handle.changed().await.is_err() {
            break;
        }
        process_pending_requests(&doc_handle).await;
    }
}

async fn process_pending_requests(doc_handle: &DocHandle) {
    let (_should_exit, req) = doc_handle.with_doc_mut(|agent| {
        if agent.should_exit {
            return (true, None);
        }

        let pos = agent.requests.iter().position(|r| {
            matches!(r, AgentRequest::LaunchApp { .. } | AgentRequest::CloseApp { .. })
        });
        let req = pos.map(|idx| agent.requests.remove(idx));
        (false, req)
    });

    match req {
        Some(AgentRequest::LaunchApp { id, splash_body }) => {
            doc_handle.with_doc_mut(|agent| {
                agent.mini_apps.insert(
                    id.clone(),
                    MiniApp {
                        splash_body,
                        state: HashMap::new(),
                    },
                );
                agent.responses.push(AgentResponse::AppLaunched { id });
            });
        }
        Some(AgentRequest::CloseApp { id }) => {
            doc_handle.with_doc_mut(|agent| {
                agent.mini_apps.remove(&id);
                agent.responses.push(AgentResponse::AppClosed { id });
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::process_pending_requests;
    use crate::doc::{AgentRequest, AgentResponse};
    use crate::repo;

    #[tokio::test]
    async fn smoke_routes_launch_and_close_and_skips_inference() {
        let (_repo, doc_handle) = repo::start_repo().await;

        doc_handle.with_doc_mut(|agent| {
            agent.requests.push(AgentRequest::Inference {
                content: "compute something".to_string(),
                app_id: "app-smoke".to_string(),
            });
            agent.requests.push(AgentRequest::LaunchApp {
                id: "app-smoke".to_string(),
                splash_body: "View { width: Fill height: Fill }".to_string(),
            });
        });

        process_pending_requests(&doc_handle).await;

        doc_handle.with_doc(|agent| {
            assert!(agent.mini_apps.contains_key("app-smoke"));
            assert!(matches!(
                agent.responses.last(),
                Some(AgentResponse::AppLaunched { id }) if id == "app-smoke"
            ));

            // Router must leave inference requests for the extension loop.
            assert!(matches!(
                agent.requests.first(),
                Some(AgentRequest::Inference { app_id, .. }) if app_id == "app-smoke"
            ));
        });

        doc_handle.with_doc_mut(|agent| {
            agent.requests.push(AgentRequest::CloseApp {
                id: "app-smoke".to_string(),
            });
        });

        process_pending_requests(&doc_handle).await;

        doc_handle.with_doc(|agent| {
            assert!(!agent.mini_apps.contains_key("app-smoke"));
            assert!(matches!(
                agent.responses.last(),
                Some(AgentResponse::AppClosed { id }) if id == "app-smoke"
            ));
        });
    }
}
