# A2App Harness — Architecture & State

## Overview

`a2app_harness` is a system for running Makepad Splash apps launched by the pi coding agent. It consists of three processes:

```
┌─────────────────────┐     JSON WS      ┌─────────────────────┐    samod WS      ┌─────────────────────┐
│                     │   (port 2341)    │                     │   (port 2342)    │                     │
│   Pi Coding Agent   │ ◄──────────────► │   Rust Harness      │ ◄──────────────► │   Makepad Host      │
│   (Node.js)         │   plain JSON     │   (bridge process)  │   CRDT sync      │   (Rust binary)     │
│                     │                  │                     │   (same samod)   │                     │
│  - Local app state  │                  │  - Creates CRDT doc │                  │  - Own DocHandle    │
│  - Simple WS client │                  │  - JSON WS server   │                  │  - AgentSplash widg │
│  - No CRDT at all   │                  │  - samod WS server  │                  │  - render + resp.   │
└─────────────────────┘                  │  - Bridge loop:     │                  └─────────────────────┘
                                         │    pi ↔ doc ↔ host  │
                                         │  - Spawns makepad   │
                                         └─────────────────────┘
```

### Key Design Change

**CRDT is ONLY used between the two Rust processes** (harness and makepad-host). The pi extension communicates with the harness over a simple JSON WebSocket — no CRDT, no automerge-repo client, no cross-language sync issues.

This eliminates the bidirectional WebSocket CRDT sync problem that plagued the previous architecture (where the JS automerge-repo client could write to the doc but couldn't read harness-originated changes).

### Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| **2341** | JSON WebSocket | pi extension ↔ harness (plain JSON messages) |
| **2342** | samod WebSocket | Harness ↔ makepad-host (CRDT sync, both Rust) |

## Components

### 1. Rust Harness (`harness/src/main.rs`)

The bridge process. Runs on a background tokio runtime.

**Responsibilities:**
- Creates a samod repo with a single shared CRDT document (`AgentDoc`)
- Runs a **JSON WebSocket server** on port 2341 for the pi extension
- Runs a **samod WebSocket server** on port 2342 for the makepad host
- Spawns the makepad-host as a child process (passing doc ID and WS URL via env vars)
- Bridge loop: watches for pi JSON WS messages → writes to CRDT doc, watches CRDT doc changes → pushes to pi JSON WS

**Environment variables:**
- `HARNESS_HEADLESS=1` — skip spawning makepad-host (for testing)

### 2. Makepad Host (`makepad-host/src/main.rs`)

The Makepad UI process. Runs Makepad on the main thread, samod client on a background thread.

**Responsibilities:**
- Connects to the harness's samod WS server as a client
- Finds the shared CRDT document, stores handle in `SHARED_DOC` static
- Background thread: listens for doc changes → signals Makepad main thread via `SIGUSR1`
- Main thread: reads `pending_app` from doc → renders splash in AgentSplash widget
- AgentSplash widget: exposes `send_response()` which writes `user_response` back to the doc
- On `should_exit`: exits the process

**Environment variables (set by harness):**
- `MAKEPAD_HOST_DOC_ID` — the CRDT document ID to find
- `MAKEPAD_HOST_WS_URL` — the samod WS URL to connect to
- `MAKEPAD_HOST_READY_MARKER` — file path to write "ready" when connected

### 3. Pi Extension (`.pi/extensions/makepad/`)

A TypeScript pi extension. Uses plain WebSocket (no CRDT).

**Files:**
- `index.ts` — extension entry point, registers tools, injects system prompt
- `tools.ts` — `launch_makepad_app`, `close_makepad_app`, `list_makepad_apps` tools
- `doc-bridge.ts` — simple WS client: connects, sends JSON, receives messages
- `harness.ts` — spawns/manages the harness binary
- `types.ts` — shared type definitions
- `standard-apps.ts` — standard app templates
- `validate-splash.ts` — splash body validation

## JSON WS Protocol (pi ↔ harness, port 2341)

### Pi → Harness
```json
{"type": "launch", "app_id": "todo-1", "splash_body": "..."}
{"type": "clear", "app_id": "todo-1"}
{"type": "exit"}
```

### Harness → Pi
```json
{"type": "welcome"}
{"type": "status", "app_id": "todo-1", "status": "Launched"}
{"type": "user_response", "app_id": "todo-1", "response": "..."}
```

## Shared Document (`AgentDoc` in `shared/src/lib.rs`)

Used ONLY between harness and makepad-host (via samod CRDT sync).

```rust
pub struct AgentDoc {
    pub pending_app: Option<PendingApp>,   // app to launch
    pub extension_requests: bool,          // pi has a pending request
    pub should_exit: bool,                 // graceful shutdown
    pub user_response: Option<String>,     // splash sends data back
}
```

## Communication Flows

### pi → Harness → Makepad Host (launch app)
1. pi sends `{"type":"launch","app_id":"...","splash_body":"..."}` over JSON WS
2. Harness writes `pending_app` to CRDT doc (sets it + `status=Pending`)
3. Harness immediately updates status to `Launched` and pushes `{"type":"status"}` back to pi
4. CRDT change syncs to makepad-host over samod WS
5. Makepad-host's background thread sees the change, signals the Makepad main thread via SIGUSR1
6. Makepad reads the doc, renders the splash body in AgentSplash widget

### Makepad Host → Harness → pi (user response)
1. Splash app calls `ui.splash.send_response("some data")`
2. AgentSplash widget writes `user_response = "some data"` to the local DocHandle
3. Change syncs to the harness over samod WS
4. Harness's bridge loop sees the change, pushes `{"type":"user_response"}` to pi over JSON WS

### Shutdown
1. pi sends `{"type":"exit"}` over JSON WS (or pi exits)
2. Harness sets `should_exit = true` in the doc (triggering makepad-host to exit)
3. Harness kills the makepad-host child process
4. Harness exits

## Build

```bash
cargo build -p harness
cargo build -p makepad-host
```

Pi extension is auto-discovered from `.pi/extensions/makepad/`.

## Splash DSL Pitfalls

### Mistake: Writing Rust `impl` syntax instead of Splash DSL

On first attempt to launch a todo app, the agent wrote a Rust-style body:

```splash
impl TodoApp for MainView {
    fn build(self, ui: &mut Ui) {
        state!(ui, tasks: Vec<String> = ...);
        ui.stack(|ui| { ... });
    }
}
```

This fails because **`launch_makepad_app` expects Splash DSL, not Rust code**.

The Splash DSL is a declarative domain-specific language parsed by Makepad's built-in splash parser. Key differences:

| Rust Makepad | Splash DSL |
|---|---|
| `impl View for MyApp { fn build(...) }` | No impl — just top-level `let`, `fn`, and widget trees |
| `state!(ui, ...)` | `let counter = 0` (plain variables, no macro) |
| `ui.label(...)` closures | `Label{text: "..."}` declarative widget literals |
| `:=` for binding closures | `:=` for naming widgets (e.g. `my_label := Label{...}`) |
| Pattern matching / `match` | `if`/`else` only |

**Always read `.pi/extensions/makepad/prompts/makepad-environment.md` first** — it contains the authoritative Splash DSL rules, syntax requirements, and a working example. Then check `standard-apps.ts` (`.pi/extensions/makepad/standard-apps.ts`) for additional working templates.

### Mistake: Omitting required `splash_body` parameter

The `standard_app` parameter on `launch_makepad_app` is informational/optional — `splash_body` is always required. To use a standard template, copy its `.splashBody` string from `standard-apps.ts`.

## Test

```bash
# Rust integration test (headless harness, no makepad-host UI)
cargo test -p harness --test integration_smoke

# TypeScript integration test (requires running harness + makepad-host)
cd .pi/extensions/makepad && npm test
```


## End of task flow

- At the end of a task, suggest a commit message to the user, based on the current diff.