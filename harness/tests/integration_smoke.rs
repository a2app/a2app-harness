use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;

use autosurgeon::{hydrate, reconcile};
use reqwest::Client;
use samod::{ConnDirection, Repo};
use samod_core::DocumentId;
use shared::{AgentDoc, AgentRequest, AgentResponse, DOC_ID_PORT, WS_PORT};
use tempfile::TempDir;
use tokio::time::{sleep, timeout, Duration, Instant};

const SMOKE_APP_ID: &str = "smoke-todo";
const TODO_SPLASH_BODY: &str = r#"let todos = [
    {text: "Buy milk" done: false}
    {text: "Write tests" done: false}
    {text: "Ship CRDT splash" done: true}
]
let max_todos = 5

fn remaining_count(){
    let count = 0
    for todo in todos {
        if !todo.done count += 1
    }
    count
}

fn sync_status(){
    ui.todo_status.set_text(remaining_count() + " remaining / " + todos.len() + " total")
}

fn sync_row_0(){
    if 0 < todos.len() {
        let todo = todos[0]
        let marker = "[ ]"
        if todo.done { marker = "[x]" }
        ui.todo_row_0.marker.set_text(marker)
        ui.todo_row_0.label.set_text(todo.text)
    } else {
        ui.todo_row_0.marker.set_text(".")
        ui.todo_row_0.label.set_text("Empty slot")
    }
}

fn sync_row_1(){
    if 1 < todos.len() {
        let todo = todos[1]
        let marker = "[ ]"
        if todo.done { marker = "[x]" }
        ui.todo_row_1.marker.set_text(marker)
        ui.todo_row_1.label.set_text(todo.text)
    } else {
        ui.todo_row_1.marker.set_text(".")
        ui.todo_row_1.label.set_text("Empty slot")
    }
}

fn sync_row_2(){
    if 2 < todos.len() {
        let todo = todos[2]
        let marker = "[ ]"
        if todo.done { marker = "[x]" }
        ui.todo_row_2.marker.set_text(marker)
        ui.todo_row_2.label.set_text(todo.text)
    } else {
        ui.todo_row_2.marker.set_text(".")
        ui.todo_row_2.label.set_text("Empty slot")
    }
}

fn sync_row_3(){
    if 3 < todos.len() {
        let todo = todos[3]
        let marker = "[ ]"
        if todo.done { marker = "[x]" }
        ui.todo_row_3.marker.set_text(marker)
        ui.todo_row_3.label.set_text(todo.text)
    } else {
        ui.todo_row_3.marker.set_text(".")
        ui.todo_row_3.label.set_text("Empty slot")
    }
}

fn sync_row_4(){
    if 4 < todos.len() {
        let todo = todos[4]
        let marker = "[ ]"
        if todo.done { marker = "[x]" }
        ui.todo_row_4.marker.set_text(marker)
        ui.todo_row_4.label.set_text(todo.text)
    } else {
        ui.todo_row_4.marker.set_text(".")
        ui.todo_row_4.label.set_text("Empty slot")
    }
}

fn sync_rows(){
    sync_row_0()
    sync_row_1()
    sync_row_2()
    sync_row_3()
    sync_row_4()
    sync_status()
}

fn add_todo(text){
    let clean = ("" + text).trim()
    if clean == "" { return }
    if todos.len() >= max_todos {
        ui.todo_status.set_text("List is full (5 max)")
        return
    }
    todos.push({text: clean done: false})
    ui.todo_input.set_text("")
    sync_rows()
}

fn toggle_todo(index){
    if index >= todos.len() { return }
    let next_done = !todos[index].done
    todos[index] += {done: next_done}
    sync_rows()
}

fn delete_todo(index){
    if index >= todos.len() { return }
    todos.remove(index)
    sync_rows()
}

fn clear_done(){
    todos.retain(|todo| !todo.done)
    sync_rows()
}

let TodoRow = RoundedView{
    width: Fill height: Fit
    padding: Inset{top: 8 bottom: 8 left: 12 right: 12}
    flow: Right spacing: 10
    align: Align{y: 0.5}
    new_batch: true
    draw_bg.color: #x2a2a3a
    draw_bg.border_radius: 8.0

    marker := Label{text: "[ ]" width: 24 draw_text.color: #x8fb7ff}
    label := Label{text: "task" width: Fill draw_text.color: #ddd}
    toggle := Button{text: "Toggle" width: 70 height: 30 on_click: || {}}
    delete := Button{text: "Delete" width: 70 height: 30 on_click: || {}}
}

RoundedView{
    width: Fill height: Fit
    flow: Down spacing: 10
    padding: 16
    new_batch: true
    draw_bg.color: #x1e1e2e
    draw_bg.border_radius: 10.0

    Label{text: "Todo" draw_text.color: #fff draw_text.text_style.font_size: 14}

    View{
        width: Fill height: Fit
        flow: Right spacing: 8
        align: Align{y: 0.5}
        todo_input := TextInput{
            width: Fill height: 34
            empty_text: "Add task"
            on_return: |text| add_todo(text)
        }
        Button{text: "Add" width: 70 height: 34 on_click: || add_todo(ui.todo_input.text())}
    }

    View{
        width: Fill height: Fit
        flow: Down spacing: 6
        todo_row_0 := TodoRow{toggle.on_click: || toggle_todo(0) delete.on_click: || delete_todo(0)}
        todo_row_1 := TodoRow{toggle.on_click: || toggle_todo(1) delete.on_click: || delete_todo(1)}
        todo_row_2 := TodoRow{toggle.on_click: || toggle_todo(2) delete.on_click: || delete_todo(2)}
        todo_row_3 := TodoRow{toggle.on_click: || toggle_todo(3) delete.on_click: || delete_todo(3)}
        todo_row_4 := TodoRow{toggle.on_click: || toggle_todo(4) delete.on_click: || delete_todo(4)}
    }

    View{
        width: Fill height: Fit
        flow: Right spacing: 8
        align: Align{y: 0.5}
        todo_status := Label{text: "" width: Fill draw_text.color: #aaa}
        Button{text: "Clear Done" width: 110 on_click: || clear_done()}
    }

    sync_rows()
}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smoke_launches_splash_app_over_full_stack() {
    ensure_makepad_host_built();

    let temp_home = TempDir::new().expect("create temp HOME for isolated repo state");
    let harness_bin = std::env::var("CARGO_BIN_EXE_harness").expect("cargo test should provide harness binary");
    let makepad_host_bin = workspace_root().join("target/debug/makepad-host");

    let mut harness = Command::new(&harness_bin)
        .env("HOME", temp_home.path())
        .env("MAKEPAD_HOST_BINARY", &makepad_host_bin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn harness process");

    let result = run_smoke_client_flow().await;

    request_harness_shutdown().await;
    terminate_child(&mut harness);

    result.expect("integration smoke flow should succeed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a desktop session; run with cargo test -p harness smoke_windowed -- --ignored --nocapture"]
async fn smoke_windowed_launches_splash_app_over_full_stack() {
    ensure_makepad_host_built();

    let temp_home = TempDir::new().expect("create temp HOME for isolated repo state");
    let harness_bin = std::env::var("CARGO_BIN_EXE_harness").expect("cargo test should provide harness binary");
    let makepad_host_bin = workspace_root().join("target/debug/makepad-host");
    let marker_file = temp_home.path().join("window-ready.marker");

    let mut harness = Command::new(&harness_bin)
        .env("HOME", temp_home.path())
        .env("MAKEPAD_HOST_BINARY", &makepad_host_bin)
        .env("MAKEPAD_HOST_WINDOWED", "1")
        .env("MAKEPAD_HOST_WINDOW_MARKER", &marker_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn harness process");

    let marker_result = wait_for_window_marker(&marker_file, Duration::from_secs(20)).await;
    let flow_result = run_smoke_client_flow().await;

    request_harness_shutdown().await;
    terminate_child(&mut harness);

    marker_result.expect("windowed makepad host should create window marker");
    flow_result.expect("windowed integration smoke flow should succeed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smoke_extension_typescript_launches_todo_via_automerge_repo() {
    ensure_makepad_host_built();
    ensure_extension_integration_deps();

    let temp_home = TempDir::new().expect("create temp HOME for isolated repo state");
    let harness_bin = std::env::var("CARGO_BIN_EXE_harness").expect("cargo test should provide harness binary");
    let makepad_host_bin = workspace_root().join("target/debug/makepad-host");

    let mut harness = Command::new(&harness_bin)
        .env("HOME", temp_home.path())
        .env("MAKEPAD_HOST_BINARY", &makepad_host_bin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn harness process");

    let ts_status = Command::new("npm")
        .args(["run", "test:integration", "--silent"])
        .current_dir(extension_dir())
        .env("SMOKE_TIMEOUT_MS", "30000")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .expect("run extension TypeScript integration script");

    request_harness_shutdown().await;
    terminate_child(&mut harness);

    assert!(
        ts_status.success(),
        "extension TypeScript integration script failed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a desktop session; run with cargo test -p harness smoke_windowed_extension -- --ignored --nocapture"]
async fn smoke_windowed_extension_typescript_launches_todo_via_automerge_repo() {
    ensure_makepad_host_built();
    ensure_extension_integration_deps();

    let temp_home = TempDir::new().expect("create temp HOME for isolated repo state");
    let harness_bin = std::env::var("CARGO_BIN_EXE_harness").expect("cargo test should provide harness binary");
    let makepad_host_bin = workspace_root().join("target/debug/makepad-host");
    let marker_file = temp_home.path().join("window-ready.marker");

    let mut harness = Command::new(&harness_bin)
        .env("HOME", temp_home.path())
        .env("MAKEPAD_HOST_BINARY", &makepad_host_bin)
        .env("MAKEPAD_HOST_WINDOWED", "1")
        .env("MAKEPAD_HOST_WINDOW_MARKER", &marker_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn harness process");

    let marker_result = wait_for_window_marker(&marker_file, Duration::from_secs(20)).await;
    let ts_status = if marker_result.is_ok() {
        Some(
            Command::new("npm")
                .args(["run", "test:integration", "--silent"])
                .current_dir(extension_dir())
                .env("SMOKE_TIMEOUT_MS", "30000")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .expect("run extension TypeScript integration script"),
        )
    } else {
        None
    };

    request_harness_shutdown().await;
    terminate_child(&mut harness);

    marker_result.expect("windowed makepad host should create window marker");
    assert!(
        ts_status
            .expect("windowed extension script should run after marker appears")
            .success(),
        "windowed extension TypeScript integration script failed"
    );
}

async fn run_smoke_client_flow() -> Result<(), String> {
    let doc_id = wait_for_doc_id(Duration::from_secs(20)).await?;

    let repo = Repo::build_tokio().load().await;
    let ws_url = format!("ws://127.0.0.1:{WS_PORT}/sync");
    let (socket, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|e| format!("connect websocket: {e}"))?;

    let connection = repo
        .connect_tungstenite(socket, ConnDirection::Outgoing)
        .map_err(|e| format!("attach websocket to samod repo: {e:?}"))?;
    connection
        .handshake_complete()
        .await
        .map_err(|e| format!("samod handshake failed: {e:?}"))?;

    let parsed_doc_id = DocumentId::from_str(doc_id.trim())
        .map_err(|e| format!("parse doc id '{doc_id}': {e}"))?;

    let doc_handle = wait_for_doc_handle(&repo, parsed_doc_id).await?;

    doc_handle.with_document(|doc| {
        let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
        agent.requests.push(AgentRequest::LaunchApp {
            id: SMOKE_APP_ID.to_string(),
            splash_body: TODO_SPLASH_BODY.to_string(),
        });
        let mut tx = doc.transaction();
        reconcile(&mut tx, &agent).expect("reconcile launch request");
        tx.commit();
    });

    wait_for_app_launch(&doc_handle).await
}

async fn wait_for_app_launch(doc_handle: &samod::DocHandle) -> Result<(), String> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        let launched = doc_handle.with_document(|doc| {
            let agent: AgentDoc = hydrate(doc).unwrap_or_default();
            let has_response = agent.responses.iter().any(|response| {
                matches!(response, AgentResponse::AppLaunched { id } if id == SMOKE_APP_ID)
            });
            let has_app = agent
                .mini_apps
                .get(SMOKE_APP_ID)
                .map(|app| app.splash_body.contains("Todo"))
                .unwrap_or(false);
            has_response && has_app
        });

        if launched {
            return Ok(());
        }

        sleep(Duration::from_millis(100)).await;
    }

    Err("timed out waiting for AppLaunched response and mini_apps state".to_string())
}

async fn wait_for_doc_id(max_wait: Duration) -> Result<String, String> {
    let client = Client::new();
    let url = format!("http://127.0.0.1:{DOC_ID_PORT}/doc_id");
    let started = Instant::now();

    while started.elapsed() < max_wait {
        match client.get(&url).send().await {
            Ok(resp) => match resp.text().await {
                Ok(body) if !body.trim().is_empty() => return Ok(body),
                Ok(_) => {}
                Err(err) => return Err(format!("read doc_id body: {err}")),
            },
            Err(_) => {}
        }
        sleep(Duration::from_millis(100)).await;
    }

    Err("timed out waiting for /doc_id".to_string())
}

async fn wait_for_doc_handle(repo: &Repo, doc_id: DocumentId) -> Result<samod::DocHandle, String> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        match repo.find(doc_id.clone()).await {
            Ok(Some(handle)) => return Ok(handle),
            Ok(None) => sleep(Duration::from_millis(100)).await,
            Err(err) => return Err(format!("repo.find failed: {err:?}")),
        }
    }

    Err("timed out waiting for shared document handle".to_string())
}

async fn request_harness_shutdown() {
    let repo = Repo::build_tokio().load().await;
    let ws_url = format!("ws://127.0.0.1:{WS_PORT}/sync");

    let connect = timeout(Duration::from_secs(2), tokio_tungstenite::connect_async(&ws_url)).await;
    let Ok(Ok((socket, _))) = connect else {
        return;
    };

    let Ok(connection) = repo.connect_tungstenite(socket, ConnDirection::Outgoing) else {
        return;
    };

    if connection.handshake_complete().await.is_err() {
        return;
    }

    let Ok(doc_id) = wait_for_doc_id(Duration::from_secs(2)).await else {
        return;
    };
    let Ok(doc_id) = DocumentId::from_str(doc_id.trim()) else {
        return;
    };
    let Ok(doc_handle) = wait_for_doc_handle(&repo, doc_id).await else {
        return;
    };

    doc_handle.with_document(|doc| {
        let mut agent: AgentDoc = hydrate(doc).unwrap_or_default();
        agent.should_exit = true;
        let mut tx = doc.transaction();
        let _ = reconcile(&mut tx, &agent);
        tx.commit();
    });
}

fn terminate_child(child: &mut Child) {
    if let Ok(Some(_)) = child.try_wait() {
        return;
    }

    let _ = child.kill();
    let _ = child.wait();
}

fn ensure_makepad_host_built() {
    let status = Command::new("cargo")
        .args(["build", "-p", "makepad-host"])
        .current_dir(workspace_root())
        .status()
        .expect("run cargo build -p makepad-host");
    assert!(status.success(), "failed to build makepad-host binary");
}

fn ensure_extension_integration_deps() {
    let node_modules = extension_dir().join("node_modules");
    if node_modules.exists() {
        return;
    }

    let status = Command::new("npm")
        .args(["install", "--no-audit", "--no-fund"])
        .current_dir(extension_dir())
        .status()
        .expect("install extension npm dependencies");

    assert!(
        status.success(),
        "failed to install extension npm dependencies"
    );
}

async fn wait_for_window_marker(marker: &Path, max_wait: Duration) -> Result<(), String> {
    let started = Instant::now();
    while started.elapsed() < max_wait {
        if marker.exists() {
            let body = fs::read_to_string(marker)
                .map_err(|e| format!("read window marker: {e}"))?;
            if body.contains("window-ready") {
                return Ok(());
            }
            if body.contains("window-error:") {
                return Err(format!("window bootstrap error: {}", body.trim()));
            }
        }
        sleep(Duration::from_millis(100)).await;
    }

    Err("timed out waiting for window marker from makepad host".to_string())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("harness crate should have workspace parent")
        .to_path_buf()
}

fn extension_dir() -> PathBuf {
    workspace_root().join(".pi/extensions/makepad")
}
