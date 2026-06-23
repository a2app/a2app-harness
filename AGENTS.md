# A2App Harness — Architecture & State

## Overview

`a2app_harness` runs Makepad Splash apps launched by the pi coding agent. Three processes:

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

**Key design:** CRDT is ONLY used between the two Rust processes (harness and makepad-host). The pi extension uses a simple JSON WebSocket — no CRDT, no cross-language sync issues.

### Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| **2341** | JSON WebSocket | pi extension ↔ harness |
| **2342** | samod WebSocket | Harness ↔ makepad-host (CRDT sync) |

## Components

### 1. Rust Harness (`harness/src/main.rs`)

Background tokio process. Creates a samod repo with a single shared CRDT document (`AgentDoc`), runs JSON WS server on :2341 and samod WS server on :2342, spawns makepad-host as child, and bridges messages between pi and the CRDT doc.

Env: `HARNESS_HEADLESS=1` — skip spawning makepad-host (for testing).

### 2. Makepad Host (`makepad-host/src/main.rs`)

The Makepad UI process. Connects to harness samod WS, finds the shared document, renders splash in `AgentSplash` widget. Background thread listens for doc changes → signals main thread via `SIGUSR1`.

AgentSplash injects a hidden `__pi_response := Label{text:""}` into every splash body. Apps call `ui.__pi_response.set_text("...")` to write data back.

Env vars (set by harness): `MAKEPAD_HOST_DOC_ID`, `MAKEPAD_HOST_WS_URL`, `MAKEPAD_HOST_READY_MARKER`.

### 3. Pi Extension (`.pi/extensions/makepad/`)

TypeScript extension. Key files:
- `tools.ts` — `launch_makepad_app`, `close_makepad_app`, `list_makepad_apps`, `check_debug_app`, `inspect_makepad_doc`, `wait_for_response`
- `doc-bridge.ts` — WebSocket client, event buffer
- `harness.ts` — spawns/manages the harness binary
- `validate-splash.ts` — splash body pre-validation

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

## Communication Flows

### Launch App
1. pi sends `{"type":"launch","app_id":"...","splash_body":"..."}` over JSON WS
2. Harness writes `pending_app` to CRDT doc (Pending → Launched)
3. CRDT syncs to makepad-host over samod WS
4. Makepad-host renders splash in AgentSplash widget on next Draw event

### User Response (splash → pi) — Automatic Agent Wake-Up
1. Splash app calls `ui.__pi_response.set_text("data")` in any `on_click` handler
2. AgentSplash detects the label text changed → writes `user_response` to CRDT doc
3. AgentSplash also increments `user_response_version` before writing
4. Harness bridge loop compares version number (not value) to detect changes
5. Harness forwards `{"type":"user_response","app_id":"...","response":"..."}` to pi
6. Pi extension buffers the event (per-type Map) in `doc-bridge.ts`
7. **Automatic wake-up**: The extension's `index.ts` `onMessage` handler detects
   the `user_response` message and calls `pi.sendUserMessage(notification, {deliverAs: "followUp"})`
   which injects a user message into the pi session, triggering a new agent turn.
   **No polling, no timeouts, no blocking.**
8. The agent wakes up and can use `inspect_makepad_doc` or other tools to check
   the app state and take action.

> **Fallback**: If `pi.sendUserMessage()` fails (e.g., session not ready), the
> response remains buffered in `doc-bridge.ts` and can be retrieved via
> `inspect_makepad_doc`.

### Shutdown
1. pi sends `{"type":"exit"}` or pi exits
2. Harness sets `should_exit = true` in the doc
3. Harness kills makepad-host child process and exits

## Shared Document (`AgentDoc` in `shared/src/lib.rs`)

```rust
pub struct AgentDoc {
    pub pending_app: Option<PendingApp>,   // app to launch
    pub extension_requests: bool,
    pub should_exit: bool,
    pub user_response: Option<String>,     // splash sends data back
    /// Monotonically increasing version counter for user_response.
    /// Incremented by makepad-host on each write so the bridge loop
    /// can detect same-value responses (e.g. toggle stays "true").
    pub user_response_version: u64,
    pub error_message: Option<String>,     // rendering error
    pub debug_command: Option<DebugCommand>,
    pub debug_response: Option<String>,
}
```

CRDT is in-memory only — no disk persistence. Restarting always starts clean.

## Debug System (`check_debug_app`)

Debug commands flow: pi → harness → CRDT doc → makepad-host → response back.

### Parameters

| Parameter | Type | Purpose |
|-----------|------|---------|
| `app_id` | optional string | App to debug (defaults to current) |
| `retry_splash_body` | optional string | Re-launch with corrected body |
| `debug_command` | optional string | One of: `widget_dump`, `widget_snapshot`, `widget_query`, `click`, `type_text` |
| `debug_params` | optional string | JSON-encoded params |
| `timeout_seconds` | optional number | Max wait (default 10, max 30) |

### Debug Commands

| Command | Params | Description |
|---------|--------|-------------|
| `widget_dump` | `"{}"` | Compact text tree of all widgets |
| `widget_snapshot` | `"{}"` | Full JSON array with id, widget_type, x, y, width, height, text, value, checked, visible, enabled |
| `widget_query` | query string | `"id:my_button"` or `"type:Button"` — returns matching positions |
| `click` | `{"x":100,"y":200}` | Simulate MouseDown+MouseUp at coordinates |
| `type_text` | raw string | Fill the first TextInput found |

### How It Works

1. pi sends `{"type":"debug",...}` → harness sets `pending_interaction` flag (for click/type_text), writes `debug_command` to CRDT doc
2. Bridge loop detects `pending_interaction` → skips one iteration (the debug_command write has stale user_response), waits for the splash-processed response
3. makepad-host receives doc change via Signal → `process_debug_commands()` executes:
   - **Read-only** (`widget_dump`, `snapshot`, `query`): use `cx.widget_tree()` API
   - **`click`**: stores `(x,y)` in `pending_click`; dispatched on next Signal/Draw event via `splash.handle_event()` as synthetic MouseDown+MouseUp (bypasses Window — splash content is orphaned from widget tree)
   - **`type_text`**: `walk_widgets_set_text()` recursively walks children, fills first TextInput, stops
4. Result written to `debug_response` on doc → harness forwards to pi and clears

### Event Ordering in `handle_event`

```rust
fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
    // Pre-dispatch: synthetic input before UI processes events
    if matches!(event, Event::Signal | Event::Draw(_)) {
        self.dispatch_pending_type_text(cx);
        self.dispatch_pending_click(cx);
    }
    // Apply deferred UI updates on Draw (before rendering)
    if matches!(event, Event::Draw(_)) {
        self.apply_pending_updates(cx);
    }
    self.ui.handle_event(cx, event, &mut Scope::empty());
    match event {
        Event::Startup => self.sync_from_doc(cx),
        Event::Signal => {
            self.sync_from_doc(cx);          // read doc → store PendingUiUpdate
            self.process_debug_commands(cx);
            self.apply_pending_updates(cx);  // apply close/clear immediately
        }
        _ => {}
    }
}
```

UI updates from doc changes are **deferred**: `sync_from_doc` on Signal stores `PendingUiUpdate`; `apply_pending_updates` applies on Draw (and end of Signal for close/clear). Early-return check (comparing `last_app_id`, `last_splash_body`, `last_error_msg`) prevents unnecessary updates on idle Signals — CPU stays at ~1.7%.

### Splash Subtree Orphan Issue

Splash content widgets have `parent = -1` in the widget tree graph. This means:
- `WidgetTree::find_within()` won't find splash content widgets — they're not in Root's subtree
- `widget_snapshot` DOES include them (iterates full dense index)
- `widget_dump` shows them with parent `-1`
- `click` dispatches directly to `splash.handle_event()`, not through Root/Window
- `type_text` walks `try_children()` on the splash's `WidgetRef` directly
- **Always use coordinates from the dump/snapshot for clicks** — widget_id lookups via `find_within` fail

### First Use Pattern (Standard Interaction Workflow)

1. **Launch**: `launch_makepad_app app_id="my-app" splash_body="..."` 
2. **Snapshot**: `check_debug_app debug_command=widget_snapshot debug_params="{}"` — find orphaned widgets at bottom (`"window_id": ""`)
3. **Calculate click center**: `x + w/2, y + h/2`
4. **Click**: `check_debug_app debug_command=click debug_params='{"x":85,"y":254}'`
5. **Verify**: Use `inspect_makepad_doc` to read `user_response`, or re-snapshot
6. **For TextInput**: `type_text` FIRST (fills first TextInput found — may be the `source` editor, not the splash's TextInput), then click a button that reads `ui.<name>.text()`

**CRITICAL: Always take a fresh snapshot before each click** — orphaned coordinates shift after layout changes (e.g., adding list items moves buttons down).

**Use `inspect_makepad_doc` for response** — `wait_for_response` may time out if the response arrived before the listener was set up. `inspect_makepad_doc` is synchronous.

### Known Interaction Issues

**Coordinates shift after layout changes**
When content grows (e.g., items added to a list via `set_text()`), the splash container height changes and all subsequent widgets shift downward. The orphan coordinates from the initial snapshot become stale. **Always take a fresh snapshot before each click** if the UI has changed since the last snapshot.

**`type_text` fills the first TextInput within splash children**
The `type_text` command calls `walk_widgets_set_text(splash, ...)` which walks the splash widget's own children recursively. This means it only ever searches orphan splash widgets — it will **not** accidentally fill the makepad-host `source` editor. However, if the splash body itself contains multiple TextInputs, it fills the first one found (breadth-first walk order). To verify which input was filled, check the `value` field of orphan TextInputs in the widget_snapshot.

### Rendering Error Handling

When splash body fails to render:
1. Makepad renders dark-red error fallback ("Splash app could not be rendered")
2. `error_message` is written to CRDT doc
3. Harness forwards `{"type":"error","app_id":"...","message":"..."}` to pi
4. The launch tool has a 1.5s debounce window after receiving `status=Launched` to collect any error messages. Errors persist in a `lastErrors` map per app_id.

## Splash DSL Guide

### Key Rules

- **Every container MUST have `height: Fit`** — most common failure mode
- `ui` object is built-in; do NOT declare it with `:=`
- **`for` loops render widgets at build time only** — array changes do NOT re-render. Use `set_text()` for dynamic content.
- **Functions with `for` loops return empty strings** when called from `on_click` — inline string building instead
- **`as int` type casting produces NaN** — use string display + `set_text()` only
- **Colons inside string arguments work correctly** — `"Time: 2:30"` is fine
- Every `TextInput` must have a fixed numeric height (e.g. `34`)
- No `on_render` in embedded apps

### Correct Pattern: Dynamic List Display (Replaces `for` Loops)

```splash
let task_count = 0
task_display := Label{text:"" font_size:14.0}
ButtonFlat{text:"Add" on_click:||{
  let t = ui.task_input.text()
  if t != "" {
    task_count = task_count + 1
    let current = ui.task_display.text()
    if current == " " { current = "" }
    if current != "" { current = current + "\n" }
    ui.task_display.set_text(current + task_count + ". " + t)
    ui.task_input.set_text("")
  }
}}
```

### Correct Pattern: Sequential Digit Input (Calculator)

```splash
let a = 0
ButtonFlat{text:"7" on_click:||{a = a*10+7; ui.display.set_text("" + a)}}
```

### Widget Availability

**Available:** View, RoundedView, Label, TextInput, LinkLabel, Button, ButtonFlat, ButtonFlatter, Slider, CheckBox, CheckBoxFlat, RadioButton, RadioButtonFlat, ToggleFlat, DropDown, TabBar, Tab, PopupMenu, ScrollBar, ScrollBars, LoadingSpinner, Hr, Vr, Icon

**NOT available (silently fail):** Stack, Divider, ProgressBar, IconButton, ToggleButton, Image, ListView, Grid, ColorPicker, ScrollPair

| Wanted | Not Available | Use Instead |
|--------|--------------|-------------|
| Divider line | `Divider` | `Hr{height:1 width:Fill}` |
| Progress bar | `ProgressBar` | `Slider{value:0.65 is_read_only:true}` |
| Tabbed UI | `TabBar`/`Tab` | `ButtonFlat` rows (TabBar renders zero-size) |

### Validation

Pre-validation catches: unknown widgets, multiline string literals, undeclared named references, parenthesized `if (cond)`, TextInput without fixed height, `on_render:`, top-level function calls.

Both `validate-splash.ts` and `dist/validate-splash.js` must be kept in sync (pi extension loads from `dist/`).

Similarly `harness.ts`/`dist/harness.js` and `tools.ts`/`dist/tools.js` must be kept in sync — pi loads from `dist/`.

## Verified Patterns (Tested 2026-06-22)

After the user_response version counter fix, all tests pass cleanly via extension tools.

| Pattern | Test Status | Notes |
|---------|-------------|-------|
| Simple button → `__pi_response.set_text()` | ✅ | Response arrives in `user_response` doc field |
| Counter via `let count = 0; count += 1` | ✅ | Variables persist across clicks |
| Toggle `let toggled = false; toggled = !toggled` | ✅ | Same-value responses work via version counter |
| TextInput + Button (`type_text` → click Submit) | ✅ | `type_text` walks splash children, not full tree |
| Dynamic list via `set_text()` concatenation | ✅ | Coordinates shift after items added |

*`type_text` calls `walk_widgets_set_text(splash, ...)` — it walks only the splash widget's own children, so it fills the first TextInput found within the splash content. Works reliably for splash TextInputs; use `widget_snapshot` to verify which orphan TextInput's `value` changed.

## Widget Reliability Reference

### Fully Reliable

| Widget | Capabilities | Best For |
|--------|-------------|----------|
| **`ButtonFlat`** | Click → variable write, `set_text()`, `text()`, `__pi_response.set_text()` | All interactive controls |
| **`Button`** | Same as ButtonFlat | Standard buttons |
| **`Label`** | `set_text()` updates visible text, `text()` reads back | Display values, status, dynamic list display |
| **`TextInput`** | `type_text` fills first input, `text()` reads value, `set_text()` writes | Text entry |
| **`Hr`** | Full-width line divider | Visual separation |
| **`RoundedView`** | Container with rounded corners | App root, groups |

### Splash VM Variable Scope (Correction)

**Splash VM `let` variables DO persist** across click events in the same app session. This was confirmed by testing:
- Counter: `let count = 0; count = count + 1` correctly produces `1, 2, 3, 4` across consecutive clicks
- Toggle: `let toggled = false; toggled = !toggled` persists `true` state across separate button clicks

However, **widget `checked` state** on `RadioButton`, `ToggleFlat`, `CheckBox` does NOT persist because internal post-processing discards the `on_click` scope context.

### Visual-Only State (Widget Properties)

| Widget | Visual State | Variable Persistence |
|--------|-------------|---------------------|
| **`RadioButton`** | `checked: true` in widget tree | ❌ Lost — internal post-processing discards `on_click` scope |
| **`ToggleFlat`** | `checked` visual renders | ❌ Same limitation |
| **`CheckBox`** / **`CheckBoxFlat`** | `checked: true` in widget tree | ❌ Same limitation (confirmed 2026-06-17) |

**Use `ButtonFlat` with manual toggle for persistent boolean state (VERIFIED ✅):**
```splash
let toggled = false
ButtonFlat{text:"Toggle" on_click:||{toggled = !toggled; ui.display.set_text("" + toggled)}}
ButtonFlat{text:"Submit" on_click:||{ui.__pi_response.set_text("" + toggled)}}  // ✅ "true"
```

### Correct Pattern: Dynamic List Display (Replaces `for` Loops) VERIFIED ✅

```splash
let task_count = 0
inp := TextInput{height:34}
lst := Label{text:"" font_size:14.0}
ButtonFlat{text:"Add" on_click:||{
  let t = ui.inp.text()
  if t != "" {
    task_count = task_count + 1
    let cur = ui.lst.text()
    if cur == " " { cur = "" }
    if cur != "" { cur = cur + "\n" }
    ui.lst.set_text(cur + task_count + ". " + t)
    ui.inp.set_text("")
  }
}}
ButtonFlat{text:"Done" on_click:||{ui.__pi_response.set_text(ui.lst.text())}}
```

⚠️ **Buttons shift down** as items are added to the list — always take a fresh snapshot before clicking.

### Available But Not Interactive via Synthetic Clicks

| Widget | Limitation |
|--------|-----------|
| **`Slider`** | `on_change` needs mouse drag — can't trigger via synthetic MouseDown/MouseUp |
| **`DropDown`** | Popup menu is separate overlay window — can't select items synthetically |

### Not in Build

| Widget | Behavior |
|--------|----------|
| **`TabBar`** / **`Tab`** | width=0, height=0 — no visible output |

## Known Current Limitations

| Limitation | Workaround |
|-----------|------------|
| `debug_response` may arrive repeatedly (harness forwards on each doc change until cleared) | Accept first response, ignore duplicates |
| `pending_click` is a single slot — two rapid clicks overwrite each other | Take a fresh `widget_snapshot` between clicks (each snapshot triggers a Signal cycle, letting the pending click dispatch before next one queues) |
| `wait_for_response` relies on async bridge loop — may time out even though doc has the data | Use `inspect_makepad_doc` (synchronous query) if `wait_for_response` times out |
| Widget text shows `" "` (space) instead of `""` for `__pi_response` | Use `value` field for TextInput, not `text` field |
| Stale content after rapid close+launch | Wait 1-2 seconds between close and launch |
| Debug commands freeze after ~50 ops (runtime state accumulation in makepad-host) | Kill both processes, rebuild, restart |
| Coordinates shift after layout changes (e.g., adding list items) | Always take a fresh `widget_snapshot` before each click |
| `type_text` fills first TextInput within splash children | Use `widget_snapshot` and check which orphan TextInput's `value` changed |
| `TabBar`/`DropDown` popup menus can't be tested synthetically | Use `ButtonFlat` rows for tab/option UIs |
| `RadioButton`, `ToggleFlat`, `CheckBox`/`CheckBoxFlat` variables don't persist in Splash VM | Use `ButtonFlat` with manual toggle |

### Recovery from Debug Freeze

If debug commands start returning `"No result provided"` or timing out after heavy use:
1. `pkill -f makepad-host; pkill -f harness`
2. `cargo build -p harness -p makepad-host`
3. Launch a new app

## Build

```bash
cargo build -p harness
cargo build -p makepad-host
```

Pi extension is auto-discovered from `.pi/extensions/makepad/`.

## Test

```bash
# Rust integration test (headless harness)
cargo test -p harness --test integration_smoke

# TypeScript integration test (requires running harness + makepad-host)
cd .pi/extensions/makepad && npm test
```

## Logs

Both processes output to stderr via `eprintln!`. Prefixes: `[harness]`, `[makepad-host]`, `[splash]`. makepad-host is spawned with `Stdio::inherit()`, so its logs go to the pi terminal.

## Test Walkthrough Protocol

When walking through apps step by step:
1. For each step, explain what you're about to do and what the user should see
2. **Wait for confirmation** before executing
3. Keep steps small — one interaction per confirmation
4. Always show coordinates before clicking
5. Only move to next step when user confirms current step is complete

## End of Task

At the end of a task, suggest a commit message to the user based on the current diff.
