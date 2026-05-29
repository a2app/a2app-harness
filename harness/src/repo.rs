use std::env;
use std::path::PathBuf;

use autosurgeon::{hydrate, reconcile};
use futures::Stream;
use samod::{DocHandle as SamodDocHandle, Repo};
use samod_core::{DocumentChanged, DocumentId};

use crate::doc::AgentDoc;

#[derive(Clone)]
pub struct RepoHandle {
    pub repo: Repo,
}

#[derive(Clone)]
pub struct DocHandle {
    inner: SamodDocHandle,
}

impl DocHandle {
    pub fn document_id(&self) -> &DocumentId {
        self.inner.document_id()
    }

    pub fn with_doc<T>(&self, f: impl FnOnce(&AgentDoc) -> T) -> T {
        self.inner.with_document(|doc| {
            let agent: AgentDoc = hydrate(doc).unwrap_or_default();
            f(&agent)
        })
    }

    pub fn with_doc_mut<T>(&self, f: impl FnOnce(&mut AgentDoc) -> T) -> T {
        self.inner.with_document(|doc| {
            let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
            let out = f(&mut agent);
            let mut tx = doc.transaction();
            reconcile(&mut tx, &agent).expect("reconcile agent doc");
            tx.commit();
            out
        })
    }

    pub fn changes(&self) -> impl Stream<Item = DocumentChanged> {
        self.inner.changes()
    }
}

pub async fn start_repo() -> (RepoHandle, DocHandle) {
    let storage_dir = harness_storage_dir();
    let repo = Repo::build_tokio()
        .with_storage(samod::storage::TokioFilesystemStorage::new(storage_dir))
        .load()
        .await;

    let mut initial = automerge::Automerge::new();
    {
        let mut tx = initial.transaction();
        reconcile(&mut tx, &AgentDoc::default()).expect("reconcile default agent doc");
        tx.commit();
    }

    let doc_handle = repo.create(initial).await.expect("create shared doc");

    (
        RepoHandle { repo },
        DocHandle { inner: doc_handle },
    )
}

fn harness_storage_dir() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".pi/agent/makepad-repo")
}
