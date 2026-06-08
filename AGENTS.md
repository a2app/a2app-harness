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

## Debugging Failures — Splash Rendering Diagnostic System

### How failures happen

When a Splash body fails to render on the makepad-host:

1. `AgentSplash::eval_body()` in `makepad-host/src/agent_splash.rs` tries to evaluate the body in Makepad's VM
2. If parsing fails, it renders an error fallback (dark red background, "Splash app could not be rendered")
3. It calls `report_error("Splash body could not be rendered")` which writes `error_message` to the shared CRDT doc
4. The harness bridge loop sees `error_message` and forwards `{"type":"error","app_id":"...","message":"..."}` over JSON WS to pi

### Problem (historical): Race condition

The harness writes `status → Launched` to the doc **immediately** after receiving the launch request, before the makepad-host has even received the CRDT sync. So `{"type":"status"}` arrives at pi before `{"type":"error"}`. The old code resolved the `launch_makepad_app` tool on the first status message, **silently swallowing the error**.

### Fix: 1.5s debounce window

In `tools.ts`, the launch tool now waits **1.5 seconds** after receiving `status` before resolving, collecting any `error` messages during that window. If an error arrives, the tool reports `isError: true` with the error message.

Total timeout: 12s (10s original + 2s buffer).

### Tool: `check_debug_app`

A dedicated tool for diagnosing and fixing splash rendering failures:

**Parameters:**
- `app_id` (optional) — defaults to the current running app
- `retry_splash_body` (optional) — corrected Splash body to re-launch

**Without `retry_splash_body`:** returns the app's status, any stored error, and a helpful hint.

**With `retry_splash_body`:** re-launches the app with the corrected body (same debounce window logic).

Errors persist in a `lastErrors` map keyed by `app_id`, so even if the error arrives after the tool call finishes, it's visible via `check_debug_app` or `list_makepad_apps`.

### Tool: `list_makepad_apps`

Now shows `error` field (null or string) alongside `id`, `status`, and `splash_preview`.

### Pre-validation: `validate-splash.ts`

Before sending a splash body to the harness, the extension runs it through `validateSplashBody()`. Errors are returned immediately with a descriptive message.

**Checks added to catch the "blue sliver" failure modes:**

| Check | What it catches |
|-------|----------------|
| Unknown widget names | `ScrollView`, `ListView`, misspelled widgets — anything not in `KNOWN_WIDGETS` set |
| Multiline string literals | `let x = "line1\nline2"` — Splash DSL strings can't span lines; use separate `Label` per line |
| Undeclared named references | Using `ui.foo` or `foo.text` where `foo` was never declared with `:=` |
| Parenthesized `if (cond)` | Splash uses `if cond { }` without parens |
| `TextInput` without fixed height | Must use numeric height like `34` |
| `text/height: Fit` on TextInput | Must use numeric height |
| `on_render:` | Destabilizes embedded apps |
| Top-level function calls | `sync_rows()` at top level—root must be a widget tree |

**`KNOWN_WIDGETS` set** (in `validate-splash.ts`):
```
View, RoundedView, Label, TextInput, Button, ButtonFlat, ButtonFlatter,
Image, Stack, Window, Portal, Draw, Slider, ToggleButton, TabBar,
DropDown, ListView, Grid, Menu, Popup, ScrollBar, ScrollBars,
ScrollPair, ColorPicker, Divider, ProgressBar, IconButton
```

If you need a widget not in this list, add it to `KNOWN_WIDGETS` in `validate-splash.ts`.

### Debugging workflow

When a splash app shows a blue/blank screen or "Splash app could not be rendered":

1. Run `list_makepad_apps` to check the current app's error state
2. Run `check_debug_app` with the app_id for detailed error info
3. Review the splash body against the validation rules above
4. Fix the body and call `check_debug_app` with `retry_splash_body` set to the corrected Splash body
5. If still failing, try the approach from the working example (`agents-viewer-2`): use individual `Label` widgets per line inside `RoundedView`, no multiline strings, no unknown widgets

### Common failure patterns

| Symptom | Likely cause |
|---------|------------|
| Blue sliver (error fallback) | Splash body failed to parse — unknown widget, multiline string, or syntax error |
| "Splash app could not be rendered" toast/fallback | Same — Makepad VM rejected the body |
| Tool says "launched" but app is blank | Race condition (should be fixed by debounce) — or the body parsed but rendered empty |
| Nothing appears at all | Harness or makepad-host crashed — check terminal for `eprintln!` output |

### Logs

The harness and makepad-host both output debug info via `eprintln!` to stderr:
- Harness runs in the background, stderr goes to the `pi` terminal
- makepad-host is spawned with `Stdio::inherit()` for stderr, so its logs also go to the `pi` terminal
- `[harness]`, `[makepad-host]`, `[splash]` prefixes identify the source

If you can't see logs, check if the pi process is running in a visible terminal.

## Test

```bash
# Rust integration test (headless harness, no makepad-host UI)
cargo test -p harness --test integration_smoke

# TypeScript integration test (requires running harness + makepad-host)
cd .pi/extensions/makepad && npm test
```


## End of task flow

- At the end of a task, suggest a commit message to the user, based on the current diff.