use autosurgeon::reconcile;
use samod::{DocHandle, Repo};

use shared::AgentDoc;

/// Create a samod repo with in-memory storage, initialise the shared document,
/// and return both the repo and the document handle.
pub async fn start_repo() -> (Repo, DocHandle) {
    let repo = Repo::build_tokio().load().await;

    let mut initial = automerge::Automerge::new();
    {
        let mut tx = initial.transaction();
        reconcile(&mut tx, &AgentDoc::default()).expect("reconcile default agent doc");
        tx.commit();
    }

    let doc_handle = repo.create(initial).await.expect("create shared doc");

    (repo, doc_handle)
}
