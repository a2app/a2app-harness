use std::sync::{Arc, RwLock};

use tokio::sync::broadcast;
use uuid::Uuid;

use crate::doc::AgentDoc;

#[derive(Clone)]
pub struct RepoHandle;

#[derive(Clone)]
pub struct DocHandle {
    id: String,
    inner: Arc<RwLock<AgentDoc>>,
    changed_tx: broadcast::Sender<()>,
}

impl DocHandle {
    pub fn document_id(&self) -> &str {
        &self.id
    }

    pub fn with_doc<T>(&self, f: impl FnOnce(&AgentDoc) -> T) -> T {
        let guard = self.inner.read().expect("doc lock poisoned");
        f(&guard)
    }

    pub fn with_doc_mut<T>(&self, f: impl FnOnce(&mut AgentDoc) -> T) -> T {
        let mut guard = self.inner.write().expect("doc lock poisoned");
        let out = f(&mut guard);
        let _ = self.changed_tx.send(());
        out
    }

    pub async fn changed(&self) -> Result<(), broadcast::error::RecvError> {
        let mut rx = self.changed_tx.subscribe();
        rx.recv().await
    }
}

pub async fn start_repo() -> (RepoHandle, DocHandle) {
    let (changed_tx, _) = broadcast::channel(256);
    let doc_handle = DocHandle {
        id: Uuid::new_v4().to_string(),
        inner: Arc::new(RwLock::new(AgentDoc::default())),
        changed_tx,
    };

    // Placeholder: samod repo wiring and websocket server goes here.
    (RepoHandle, doc_handle)
}
