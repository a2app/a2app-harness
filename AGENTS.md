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
- AgentSplash widget: injects `__pi_response` hidden label; splash apps call `ui.__pi_response.set_text()` to write `user_response` back to the doc
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
{"type": "debug", "app_id": "todo-1", "command": "widget_snapshot", "params": "{}"}
{"type": "exit"}
```

### Harness → Pi
```json
{"type": "welcome"}
{"type": "status", "app_id": "todo-1", "status": "Launched"}
{"type": "user_response", "app_id": "todo-1", "response": "..."}
{"type": "debug_response", "app_id": "todo-1", "result": "..."}
```

## Debug System (`check_debug_app` tool)

The `check_debug_app` tool (extended with `debug_command`/`debug_params`)
inspects and interacts with the running Makepad Splash app. Debug commands
flow through: pi → harness → CRDT doc → makepad-host (processes) → response back.

### Parameters

| Parameter | Type | Purpose |
|-----------|------|---------|
| `app_id` | optional string | App to debug (defaults to current) |
| `retry_splash_body` | optional string | Re-launch with corrected body |
| `debug_command` | optional string | One of: `widget_dump`, `widget_snapshot`, `widget_query`, `click`, `type_text` |
| `debug_params` | optional string | JSON-encoded params for the command |
| `timeout_seconds` | optional number | Max wait for debug response (default 10, max 30) |

### Debug Commands

| Command | Params | Description |
|---------|--------|-------------|
| `widget_dump` | `"{}"` | Compact text tree: `W3 <count>` then `index parent id type x y w h` per line |
| `widget_snapshot` | `"{}"` | Full JSON array of all widgets with `id`, `widget_type`, `x`, `y`, `width`, `height`, `text`, `value`, `checked`, `visible`, `enabled` |
| `widget_query` | query string like `"id:my_button"` or `"type:Button"` | Returns matching widget positions as text lines |
| `click` | `{"x":100,"y":200}` | Simulate MouseDown+MouseUp at absolute window coordinates |
| `type_text` | raw string like `"hello"` | Inject text into the first TextInput found in the splash content |

**Important:** For `click`, use x,y coordinates from `widget_dump` or `widget_snapshot`
— `widget_id` lookup doesn't work reliably because splash content subtrees are
orphaned from the main widget tree (parent = -1). The widget dump shows absolute
window coordinates; calculate center as `x + w/2, y + h/2`.

### How It Works

1. **Pi extension** sends `{"type":"debug","app_id":"...","command":"widget_snapshot","params":"{}"}`
2. **Harness** writes `debug_command` to the shared CRDT doc
3. **Makepad-host** receives the doc change via `Event::Signal`
4. **app.rs** `process_debug_commands()` reads the command and executes it:
   - _Read-only commands_ (`widget_dump`, `snapshot`, `query`): use `cx.widget_tree()` API directly
   - _`click`_: stores coordinates in `self.pending_click`, dispatched **before** `self.ui.handle_event()`
     on the next Signal/Draw cycle as synthetic `MouseDownEvent`+`MouseUpEvent` sent
     **directly to the AgentSplash widget** via `splash.handle_event()`. This bypasses the
     Window widget entirely — necessary because splash content widgets are orphaned from
     the widget tree (parent = -1) and can't be reached via normal tree traversal from Root.
   - _`type_text`_: calls `walk_widgets_set_text()` which traverses the splash's
     `WidgetRef` children recursively via `try_children()`, finds the first
     `TextInput` widget by `borrow::<TextInput>()`, and calls `set_text()`
5. Result is written to `debug_response` on the doc
6. **Harness bridge loop** detects `debug_response`, forwards it as
   `{"type":"debug_response",...}` to pi, then clears it from the doc

### Critical: Event Ordering in `handle_event`

In `app.rs`, `dispatch_pending_type_text` and `dispatch_pending_click` run
**BEFORE** `self.ui.handle_event()` on Signal/Draw events. This ensures
synthetic input state is ready before the widget tree processes events.

Additionally, **UI updates from doc changes are deferred**: `sync_from_doc`
(which reads the shared CRDT doc) runs on Signal and stores pending changes.
These are applied on the NEXT Draw event (before the UI renders) via
`apply_pending_updates`. This ensures widget tree mutations (splash body eval,
set_text) happen during the render phase, not during event processing.

```rust
fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
    // Pre-dispatch: synthetic input before UI processes events
    if matches!(event, Event::Signal | Event::Draw(_)) {
        self.dispatch_pending_type_text(cx);  // write __pi_type_text or walk widgets
        self.dispatch_pending_click(cx);      // inject mouse events
    }

    // Apply deferred UI updates on Draw (before rendering)
    if matches!(event, Event::Draw(_)) {
        self.apply_pending_updates(cx);       // apply splash body, source, status changes
    }

    self.ui.handle_event(cx, event, &mut Scope::empty());

    match event {
        Event::Startup => { self.sync_from_doc(cx); }
        Event::Signal => {
            self.sync_from_doc(cx);            // read doc, store pending
            self.process_debug_commands(cx);   // process clicks, type_text, debug queries
            self.apply_pending_updates(cx);    // apply close/clear changes immediately
        }
        _ => {}
    }
}
```

### Deferred Update Architecture

**Motivation:** In the original architecture, `sync_from_doc` (called on Signal)
immediately called `set_text()` on widgets (splash body, source code, status line).
These widget tree mutations happened during event processing, potentially causing
timing issues with Splash VM evaluation and redraw requests.

**Solution (June 2026):** Defer UI mutations to the render phase:
1. `sync_from_doc` on Signal → reads doc → stores `PendingUiUpdate` struct
2. `apply_pending_updates` on Draw → applies stored changes before rendering
3. Also called at end of Signal handling → ensures close/clear operations render
   even if no Draw event follows immediately

**`PendingUiUpdate` fields:**
- `splash_body: Option<String>` — new splash body to render (empty string = clear)
- `source_body: Option<String>` — source code display text
- `status: Option<String>` — status line text (e.g. "App: todo-1")
- `error_msg: Option<String>` — error message to show
- `should_exit: bool` — if true, exit the process

### Splash Subtree Orphan Issue

Splash content (the inner View created by `View::script_from_value`) has
parent = -1 in the widget tree graph. This means:
- `WidgetTree::find_within(uid, path)` won't find widgets inside the splash
  content — they're not in the search root's subtree.
- `widget_snapshot` DOES include them (iterates the full dense index).
- `widget_dump` shows them with parent `-1`.
- `click` dispatches events directly to the splash widget via
  `splash.handle_event()`, which correctly routes through the actual view
  hierarchy (not the widget tree).
- `type_text` walks `try_children()` on the splash's `WidgetRef` directly
  (not the widget tree), so it finds children correctly.
- **Always use coordinates from the dump/snapshot for clicks** — widget_id
  lookups via `find_within` won't work for splash content.

### First Use Pattern

1. `check_debug_app debug_command=widget_dump debug_params="{}"`
   — discover widget IDs, positions, and types
2. Note the `x y w h` columns. Calculate click center: `x + w/2, y + h/2`
3. `check_debug_app debug_command=click debug_params='{"x":52,"y":227}'`
   — click the button at its center
4. `check_debug_app debug_command=widget_snapshot debug_params="{}"`
   — verify state changed (check `text` field of target widget)
5. `check_debug_app debug_command=type_text debug_params="hello"`
   — type into a TextInput, then click the "Show" button to verify

### Known Limitations

| Issue | Cause | Workaround |
|-------|-------|------------|
| `debug_response` may arrive repeatedly | Bridge loop forwards it on each doc change until cleared | Accept first response; ignore duplicates |
| Widget tree `find_within` fails for splash content | Splash View has parent = -1 in graph | Use coordinates from dump/snapshot for clicks; type_text walks children directly |
| Widget text may show as `" "` (space) instead of `""` | AgentSplash's `__pi_response` label initializes with space | Use `value` field for TextInput, not `text` field |
| Multiple queued clicks may stack before processing | Clicks stored in `pending_click` field | Add delays between click commands |
| Click must dispatch directly to splash, not Root | Splash content orphaned (parent=-1) | `splash.handle_event()` not `self.ui.handle_event()` |
| Synthetic events need `WindowId(0,0)` | First window gets index 0 | Use `WindowId(0, 0)` for MouseDown/MouseUp events |
| `text_input.text()` can't read Rust-set values | Splash VM reads from own cache | Use counters and `set_text()` in Splash code instead |

## Shared Document (`AgentDoc` in `shared/src/lib.rs`)

Used ONLY between harness and makepad-host (via samod CRDT sync).

```rust
pub struct AgentDoc {
    pub pending_app: Option<PendingApp>,   // app to launch
    pub extension_requests: bool,          // pi has a pending request
    pub should_exit: bool,                 // graceful shutdown
    pub user_response: Option<String>,     // splash sends data back
    pub error_message: Option<String>,     // rendering error
    pub debug_command: Option<DebugCommand>, // debug tool commands
    pub debug_response: Option<String>,    // debug tool responses
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

The splash app communicates back to the pi agent via a **hidden `__pi_response` label** that is automatically injected into every splash body. This replaces the old `ui.splash.send_response()` API.

**How it works:**
1. Splash app calls `ui.__pi_response.set_text("some data")` in any `on_click` handler
2. `AgentSplash::handle_event()` detects the label text changed → calls `write_doc_field("user_response", data)` to CRDT doc
3. Change syncs to the harness over samod WS
4. Harness's bridge loop sees the change, pushes `{"type":"user_response","app_id":"...","response":"..."}` to pi over JSON WS
5. Pi extension receives via `doc-bridge.ts` WebSocket message handler

**Splash code example:**
```splash
ButtonFlat{text:"Send" on_click:||{
    ui.__pi_response.set_text("action:count,value:42")
}}
```

The response can be any string — JSON, key=value pairs, or plain text.

**Buffer system:** Every incoming message is buffered in `doc-bridge.ts` by type, so responses that arrive between tool calls are never lost.

### Shutdown
1. pi sends `{"type":"exit"}` over JSON WS (or pi exits)
2. Harness sets `should_exit = true` in the doc (triggering makepad-host to exit)
3. Harness kills the makepad-host child process
4. Harness exits

## Two-Way Communication System

Two-way communication between the pi agent and the splash app is the core value of a2app_harness.
The splash app can not only display UI — it can **send structured responses back to the pi agent**
via an event-driven pipeline.

### Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        TWO-WAY COMMUNICATION FLOW                          │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  PI AGENT ───(1) launch_app ───► HARNESS ──(2) CRDT doc ──► MAKEPAD HOST  │
│  (tool call)      JSON WS       (bridge)   sync via         (render UI)    │
│                                     │      samod WS                        │
│                                     │                                       │
│  PI AGENT ◄───(5) user_response ────┘ ◄─(4) detect change── MAKEPAD HOST  │
│  (event)          JSON WS           (bridge loop)    ◄──(3) click btn     │
│                                                         __pi_response      │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Direction 1: Pi Agent → Makepad Host (Launch/Update)

| Step | Component | Action | Protocol |
|------|-----------|--------|----------|
| 1 | Pi tool | Calls `launch_makepad_app` with splash body | — |
| 2 | doc-bridge.ts | Sends `{"type":"launch","app_id","splash_body"}` | JSON WS :2341 |
| 3 | Harness | Writes `pending_app` to CRDT doc (Pending → Launched) | Local doc write |
| 4 | samod WS | Syncs CRDT change to makepad-host | samod WS :2342 |
| 5 | Makepad host | Receives Signal → reads doc → renders splash | — |

### Direction 2: Makepad Host → Pi Agent (Response)

| Step | Component | Action | Protocol |
|------|-----------|--------|----------|
| 1 | Splash app | User clicks button → `ui.__pi_response.set_text("...")` | — |
| 2 | AgentSplash | `handle_event()` detects label text change | — |
| 3 | AgentSplash | Calls `write_doc_field("user_response", data)` | Local doc write |
| 4 | samod WS | Syncs CRDT change (user_response field) | samod WS :2342 |
| 5 | Harness | Bridge loop sees doc change | — |
| 6 | Harness | Forwards `{"type":"user_response"}` to pi | JSON WS :2341 |
| 7 | doc-bridge.ts | Buffers event + dispatches to handlers | — |
| 8 | Tool | Handler receives `user_response` data | — |

### The `__pi_response` Mechanism

Every splash body evaluated by `AgentSplash` is automatically wrapped with a suffix that
injects a hidden label:

```rust
// In agent_splash.rs:
const SPLASH_SUFFIX: &str = "  __pi_response := Label{text:\"\"}";
```

This label is invisible (zero-size, no background) and can be written to from any
`on_click` closure in the splash body:

```splash
ButtonFlat{text:"Send" on_click:||{
    ui.__pi_response.set_text("any string data here")
}}
```

**Detection in AgentSplash::handle_event():**

```rust
let response_widget = self.widget(cx, &[id!(__pi_response)]);
if !response_widget.is_empty() {
    let current = response_widget.text();
    if current != self.last_response && !current.is_empty() {
        let new_response = current.clone();
        self.last_response = current;
        write_doc_field("user_response", new_response.clone());
    }
}
```

Key behaviors:
- Only detects **changes** (compares against `last_response`)
- Only sends **non-empty** strings (initial value is a single space `" "`)
- `last_response` is reset to `""` each time a new splash body is set

### Harness Bridge Loop (doc → pi forwarding)

In `harness/src/main.rs`, the bridge loop watches CRDT doc changes via
`doc_handle.changes()`. When it detects `user_response` has been set, it
pushes to the pi WebSocket:

```rust
if let Some(ref response) = has_response {
    let msg = HarnessToPiMsg::UserResponse {
        app_id: id.clone(),
        response: response.clone(),
    };
    let _ = bridge.lock().await.pi_tx.send(json);
}
```

### Persistent Event Buffer (doc-bridge.ts)

All incoming messages from the harness are captured in a `Map<string, HarnessMessage>`
event buffer. This ensures responses are never lost, even if they arrive between
tool calls:

```typescript
const eventBuffer: Map<string, HarnessMessage> = new Map();

export function getBufferedEvent(type: string): HarnessMessage | undefined {
  return eventBuffer.get(type);
}

export function getAllBufferedEvents(): HarnessMessage[] {
  return Array.from(eventBuffer.values());
}

export function clearEventBuffer(): void {
  eventBuffer.clear();
}
```

The buffer stores one message per type (last-write-wins), since the CRDT doc is the
source of truth and events are just notifications. Buffer can be cleared explicitly
via `clearEventBuffer()` or implicitly by tools that consume events.

### New Tools (registered in pi extension)

#### `inspect_makepad_doc`

Queries the harness for the full CRDT document state. Protocol:
- Pi sends: `{"type": "get_doc"}` over JSON WS
- Harness responds: `{"type": "doc_state", "app_id", "user_response", "error_message", "status"}`

Also checks the local event buffer for any `user_response` or `error` messages
that arrived between tool calls. Clears the buffer after reading.

**Use cases:**
- Check if a splash app has sent a response
- Check for render errors
- See what app is currently running

#### `wait_for_response`

An event-driven listener that blocks until a `user_response` is received from the
splash app. This enables a **service worker** pattern:

```
1. Agent launches a splash app with ui.__pi_response.set_text() buttons
2. Agent calls wait_for_response with a timeout
3. wait_for_response registers a handler and awaits
4. When user clicks a button, the response flows through the full pipeline
5. wait_for_response resolves with the response data
```

**Parameters:**
- `app_id` (optional) — filter by app (defaults to current)
- `timeout_seconds` (optional) — max wait time (default 30, max 120)
- `clear_buffer` (optional) — clear buffered responses before waiting (default true)

**Returns:**
- `app_id` — the app that sent the response
- `response` — the string data sent via `__pi_response.set_text()`
- `source` — `"live"` (just arrived) or `"buffered"` (was already in the buffer)

**Example usage flow:**
```splash
// Launch this app:
ButtonFlat{text:"Submit" on_click:||{
    ui.__pi_response.set_text("form:submitted,name:Alice")
}}
```

Then from the agent: `wait_for_response app_id="my-app"`

### Verified End-to-End (2026-06-16)

| Test | Result |
|------|--------|
| Launch app with __pi_response buttons | ✅ Renders correctly |
| Click "Send Hello Back" → `__pi_response` text changes | ✅ `"Hello from the splash app!"` |
| Click "Action: Count" → `__pi_response` text changes | ✅ `"action:count,value:42"` |
| Click "Action: Data" → `__pi_response` text changes | ✅ `"action:data,temperature:72,humidity:45,status:ok"` |
| Multiple sequential clicks update correctly | ✅ Each click overwrites previous response |
| Event buffer captures responses between tool calls | ✅ Always-on, per-type Map |
| `inspect_makepad_doc` queries doc state | ✅ New tool (needs extension reload) |
| `wait_for_response` blocks on event | ✅ New tool (needs extension reload) |
| `type_text` fills only first TextInput | ✅ Fixed — `walk_widgets_set_text` stops after first match |
| Colons inside string arguments to `set_text()` | ✅ Works — `"Time: 2:30"` renders correctly, `"1:00"` sends correctly |
| Splash VM reads Rust-set TextInput values | ✅ Proven: `"A:" + ui.field_a.text()` = `"A:HelloWorld"` after `type_text` |
| Idle CPU (no debug commands) | ✅ 1.7% — no more 100% spin loop from idle Signals |

### Interactive Test Procedure

To manually verify two-way communication:

1. Launch a splash app with a button that calls `ui.__pi_response.set_text()`
2. Use `widget_snapshot` to find button coordinates
3. Click the button via `check_debug_app debug_command=click debug_params='{"x":...,"y":...}'`
4. Take another `widget_snapshot` — check that `__pi_response` label text changed
5. The harness bridge loop automatically forwards the change to the pi extension

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

**Always read `.pi/extensions/makepad/prompts/makepad-environment.md` first** — it contains the authoritative Splash DSL rules, syntax requirements, and working examples. Then check `standard-apps.ts` (`.pi/extensions/makepad/standard-apps.ts`) for additional working templates.

**Key rules discovered through testing (not in the prompt):**
- Every container **MUST** have `height: Fit` — this is the most common failure mode
- The `ui` object is built-in; you do NOT need to declare it with `:=`
- Only certain widgets exist in this Makepad build; others silently fail
- Colons inside string arguments to `ui.*.set_text()` work correctly (verified with `"Time: 2:30"` and `"1:00"`) — no false positive

### Mistake: Omitting `height: Fit` on containers

Every `View`, `RoundedView`, etc. **MUST have explicit `height: Fit`**. Without it, the container collapses to zero height and nothing visible renders inside it. This was the single most common cause of "blue rectangle with no content" during testing.

```splash
✅ RoundedView{width:Fill height:Fit flow:Down padding:16 new_batch:true ...}
❌ RoundedView{width:Fill flow:Down padding:16 ...}  ← invisible!
❌ View{padding:30 ...}  ← invisible!
```

Note that `RoundedView` does NOT have a default height of `Fit`. Every container needs it explicitly.

### Mistake: Using `Stack`, `Divider`, `ProgressBar`, `IconButton`, `ToggleButton`

These widgets do not exist in this Makepad build. They silently fail to render (no error, just no visible output). Use the available alternatives:

| Wanted | Not Available | Use Instead |
|--------|--------------|-------------|
| Divider line | `Divider` | `Hr{height:1 width:Fill}` |
| Toggle switch | `ToggleButton` | `ToggleFlat{text:"..." selected:false}` |
| Stack overlay | `Stack` | Manual positioning with `View` containers |
| Progress bar | `ProgressBar` | `Slider{value:0.65 is_read_only:true}` |

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

**`KNOWN_WIDGETS` set** (in both `validate-splash.ts` and `dist/validate-splash.js`):
```
Containers:    View, RoundedView
Text:          Label, TextInput, LinkLabel
Buttons:       Button, ButtonFlat, ButtonFlatter
Inputs:        Slider, CheckBox, CheckBoxFlat, RadioButton, RadioButtonFlat, ToggleFlat
Menus/Lists:   DropDown, TabBar, Tab, PopupMenu, ScrollBar, ScrollBars, LoadingSpinner
Decorations:   Hr, Vr, Icon
```

**NOT available** (will silently fail to render): `Stack`, `Divider`, `ProgressBar`, `IconButton`, `ToggleButton`, `Image`, `ListView`, `Grid`, `ColorPicker`, `ScrollPair`

**Note:** These widgets are from `grep` of actual widget source files. If you need a widget not in the list, verify it exists in `widgets/src/*.rs` before adding.

### Container `height: Fit` Requirement

**Every container widget (`View`, `RoundedView`, etc.) MUST have explicit `height: Fit`.**

Without it, the container collapses to 0px height and is invisible. This is the #1 cause of "blue rectangle but no content" failures.

```
✅ RoundedView{width:Fill height:Fit flow:Down padding:16 ...}
✅ View{height:Fit padding:30 ...}
❌ RoundedView{width:Fill padding:16 ...}  ← invisible!
❌ View{padding:30 ...}  ← invisible!
```

### Validator Pitfalls

- **The `ui` object is built-in in Splash DSL.** The validator must include `"ui"` in `declaredIds` to avoid false positives. Both `validate-splash.ts` AND `dist/validate-splash.js` must be updated.
- **Colons inside string arguments work correctly** (verified with `"Time: 2:30"` and `"current time is 1:00 and that's fine"`).
- **Both files need updating:** The TypeScript source (`validate-splash.ts`) and the compiled JS (`dist/validate-splash.js`) must be kept in sync. The pi extension loads from `dist/`.

### Debugging workflow

When a splash app shows a blue/blank screen or "Splash app could not be rendered":

1. Run `list_makepad_apps` to check the current app's error state
2. Run `check_debug_app` with the app_id for detailed error info
3. Review the splash body against the validation rules above
4. Fix the body and call `check_debug_app` with `retry_splash_body` set to the corrected Splash body
5. If still failing, check that:
   - All containers have explicit `height: Fit`
   - No unknown widgets are used (check widget availability list)
   - No colons in string args to `ui.*.set_text()`

### Common failure patterns

| Symptom | Likely cause |
|---------|------------|
| Blue sliver (error fallback) | Splash body failed to parse — unknown widget, multiline string, or syntax error |
| "Splash app could not be rendered" toast/fallback | Same — Makepad VM rejected the body |
| Blue/empty rectangle, no content | Container missing `height: Fit` — it collapsed to 0px |
| Tool says "launched" but nothing visible | Container has no `height: Fit`, or widget doesn't exist in this build (e.g. `Stack`, `Divider`) |
| Parser-syntax error using standard template | Colons inside string args work correctly (tested with `"Time: 2:30"` and `"current time is 1:00 and that's fine"`) |
| Nothing appears at all | Harness or makepad-host crashed — check terminal for `eprintln!` output |
| App launched but status stuck on "Pending" | Pi extension resolved on first status (Pending) before makepad-host updated to Launched; this is a known race condition

### Logs

The harness and makepad-host both output debug info via `eprintln!` to stderr:
- Harness runs in the background, stderr goes to the `pi` terminal
- makepad-host is spawned with `Stdio::inherit()` for stderr, so its logs also go to the `pi` terminal
- `[harness]`, `[makepad-host]`, `[splash]` prefixes identify the source

If you can't see logs, check if the pi process is running in a visible terminal.

## Interactivity Test Results (verified 2026-06-09)

### Within-App Interactivity

| Test | Result | Notes |
|------|--------|-------|
| Button clicks (`on_click`) | ✅ Works | State vars update, UI reflects changes |
| `ui.<name>.set_text()` | ✅ Works | Updates any widget's text |
| `ui.<name>.text()` | ✅ Works | Reads TextInput content |
| Multiple statements in closure | ✅ Works | Use `;` separator inside `{ }` |
| Functions (`fn foo(){...}`) | ✅ Works | Can call `ui.*` and functions |
| `set_interval()` / `clear_interval()` | ❌ NOT available | Not in Makepad script VM |
| `send_response()` from splash body | ❌ Not callable | Only callable from parent app code |
| App replacement | ✅ Works | New `launch` replaces old app |
| Conditional `if` rendering | ✅ Works | Works at widget level |
| `as int` type casting | ❌ Produces NaN | `val as int` on a string value gives `NaN`; use string display + `set_text()` instead |
| Inline variable in Label text | ⚠️ Static only | `Label{text:"Count: " + count}` evaluated at build time; to update, use `ui.<name>.set_text()` |
| CheckBox `checked` toggle | ✅ Works | `on_click` can toggle `selected:false` state |
| RadioButton group selection | ✅ Works | `group:1` parameter enables radio group; click selects one |
| `type_text` + button click pipeline | ✅ Works | type_text fills first TextInput; button click reads value via `ui.<name>.text()` correctly |
| `Hr{height:1 width:Fill}` divider | ✅ Works | Renders a visible horizontal rule |
| `Slider` widget renders | ✅ Renders | Present and visible, `on_change` callback fires |
| `send_response()` via `__pi_response.set_text()` | ✅ Works | Hidden label writes response to shared doc, forwarded by harness bridge |
| Deferred UI updates | ✅ Works | sync_from_doc on Signal → store pending → apply on Draw |
| Synthetic click dispatch to splash | ✅ Works | Dispatch directly to AgentSplash (not through Root/Window) |
| Close app clears visual state | ✅ Works | Empty splash body renders empty View |

### Verified Limitations

| Limitation | Evidence | Workaround |
|-----------|----------|------------|
| `as int` type conversion | `"100" as int` → `NaN°F` | Use string manipulation + `set_text()` only |
| Inline expressions in Labels | `"Score: " + score` stays at initial value | Always use `ui.<name>.set_text()` for dynamic content |
| `type_text` bypasses `on_return` | Text set directly, callback not fired | Click a button that reads `ui.<name>.text()` to process |
| ~~`text_input.text()` returns `[Error:WrongValue]`~~ | ✅ FIXED — Splash VM reads Rust-set values correctly | Proven: `"A:" + ui.field_a.text() + ",B:" + ui.field_b.text()` returned `A:HelloWorld,B:` after Rust `set_text()` |
| Closing app leaves stale view | Old content persisted when close didn't trigger Draw event | Fixed — `apply_pending_updates` now runs at end of Signal handling too (June 2026) |
| Click dispatch must go to splash directly | Splash content orphaned (parent=-1) from widget tree | Events dispatched via `splash.handle_event()`, not `self.ui.handle_event()` |
| WindowId(1,0) doesn't match actual window | First window has index 0, generation 0 | Use `WindowId(0, 0)` for synthetic events |

### Close/Clear Fix (2026-06-11 & 2026-06-12)

**Problem (2026-06-11):** When `close_makepad_app` was called, `AgentSplash::set_text("")` set `render_ok = true` but **never cleared `self.view`** — the previously rendered widget tree stayed visible.

**Fix (2026-06-11):** In `agent_splash.rs`, `eval_body()` renders an empty `View{width:Fill height:Fit}` when body is empty.

**Problem (2026-06-12):** After the deferred update architecture was introduced, `apply_pending_updates` only ran on Draw events. If no Draw event followed a close operation, the visual state wasn't cleared.

**Fix (2026-06-12):** `apply_pending_updates` now also runs at the end of `Event::Signal` handling in `handle_event()`, ensuring close/clear operations take effect immediately.

### `walk_widgets_set_text` Fix (2026-06-16)

**Problem:** `type_text` debug command filled ALL TextInput widgets with the typed text, not just the first one. The `walk_widgets_set_text` function recursively walked all children and set text on every TextInput found.

**Fix:** Added a `found` boolean flag that stops traversal after the first TextInput is found:
```rust
fn walk_widgets_set_text(widget: WidgetRef, cx: &mut Cx, text: &str) -> bool {
    if widget.borrow::<makepad_widgets::TextInput>().is_some() {
        widget.set_text(cx, text);
        return true;
    }
    let mut found = false;
    widget.try_children(&mut |_, child| {
        if !found {
            found = Self::walk_widgets_set_text(child, cx, text);
        }
    });
    found
}
```

Verified: `type_text "HelloWorld"` → only `field_a` has value "HelloWorld", `field_b` remains empty.

### Idle Signal Spiral Fix (2026-06-16)

**Problem:** `sync_from_doc` always stored a `PendingUiUpdate` on every Signal, even when nothing changed. `apply_pending_updates` called `set_text()` on the error line every time, causing continuous redraw loops and 100% CPU.

**Fix:** Added early-return check comparing current doc values against `last_app_id`, `last_splash_body`, and `last_error_msg`. If nothing changed, the function returns immediately without storing a pending update.

Verified: CPU drops to ~1.7% idle after launch, no more spin loop.

### Horizontal Layout (inner `View{flow:Right}`)

The inner View MUST have `height: Fit`:
```
✅ View{height:Fit flow:Right spacing:12 ...}
❌ View{flow:Right spacing:12 ...}  ← buttons invisible!
```

### send_response Mechanism

Splash apps can send arbitrary string data back to the pi extension by setting text on the built-in `__pi_response` label:

```splash
ButtonFlat{text:"Send" on_click:||{ui.__pi_response.set_text("my response data")}}
```

**Implementation** (in `makepad-host/src/agent_splash.rs`):
1. The `SPLASH_SUFFIX` injects a hidden `__pi_response := Label{text:""}` into every evaluated body
2. `AgentSplash::handle_event` checks this label's text on every event cycle
3. If text has changed (and is non-empty), it writes `user_response` to the shared CRDT doc
4. The harness bridge loop detects the change and forwards `{"type":"user_response","app_id":"...","response":"..."}` to pi

### Status Race Condition Fix

**Problem:** The harness used to write `status: Pending` to the doc, then the bridge loop immediately forwarded it to pi. The pi extension started its debounce timer on the first status ("Pending"), but rendering errors from makepad-host arrived after the debounce expired. Result: tools reported "Launched" even when rendering failed.

**Fix (harness):** The launch handler now writes `Pending` and then immediately writes `Launched` in a second transaction:
```rust
// First write: Pending
doc_handle.with_document(|doc| { ... status: Pending ... });
// Second write: Launched (immediately after)
doc_handle.with_document(|doc| { ... status: Launched ... });
```

**Fix (pi extension):** The tools.ts launch handler now only starts its 1.5s debounce on `msg.status === "Launched"`, not on any status message. Errors from makepad-host arrive within the debounce window because the harness writes Launched first (triggering the timer) and errors from the host arrive shortly after via CRDT sync.

### Error Handling

| Scenario | Behavior |
|----------|----------|
| Unknown widget | Caught by pre-validation before sending |
| VM eval failure | Makepad renders error fallback (dark red), sets `error_message` in doc |
| Status race condition | Fixed — harness writes Launched immediately; pi extension waits for Launched

## Test

```bash
# Rust integration test (headless harness, no makepad-host UI)
cargo test -p harness --test integration_smoke

# TypeScript integration test (requires running harness + makepad-host)
cd .pi/extensions/makepad && npm test
```


## Test Walkthrough Protocol

When a user asks for a test run / to walk through a series of apps step by step:

1. For each test step, **stop and explain** what you're about to do and what the user should see on the Makepad window.
2. **Wait for the user to confirm** before executing.
3. If user confirms → execute and show results.
4. If user rejects → debug or adjust.
5. Only move to the next step/app when user explicitly confirms the current step is complete.
6. Keep each step small — one interaction (e.g., one type_text, one click, one snapshot) per confirmation.
7. Always show coordinates before clicking.

## End of task flow

- At the end of a task, suggest a commit message to the user, based on the current diff.