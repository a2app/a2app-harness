mod agent_splash;
mod app;
mod app_host;
mod doc_agent;
mod state;

use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use state::{HostCommand, COMMAND_TX};

fn main() {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<HostCommand>();
    let _ = COMMAND_TX.set(cmd_tx);

    // Initialize host state and signal BEFORE the tokio thread starts,
    // so doc_agent::run can post window Signal events immediately.
    app_host::init_host_signal();

    std::thread::spawn(|| {
        let rt = Runtime::new().expect("create tokio runtime");
        rt.block_on(async {
            let session = doc_agent::setup_doc().await;
            doc_agent::run(session, cmd_rx).await;
        });
    });

    app_host::run_app();
}
