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

1. Launch the app with `launch_makepad_app`
2. Take a `widget_snapshot` to discover widget positions:
   `check_debug_app debug_command=widget_snapshot debug_params="{}"`
3. Find the widget you want to interact with in the JSON output. Look for orphaned widgets
   (those at the bottom with `"window_id": ""`). Calculate click center: `x + w/2, y + h/2`
4. Click the button:
   `check_debug_app debug_command=click debug_params='{"x":85,"y":254}'`
5. Take another snapshot to verify state changed:
   `check_debug_app debug_command=widget_snapshot debug_params="{}"`
6. For TextInput interaction, use `type_text` FIRST, then click a button to read the value:
   `check_debug_app debug_command=type_text debug_params="Hello"`
   `check_debug_app debug_command=click debug_params='{"x":79,"y":258}'`
7. **Important: Always take a fresh snapshot before each click.** Window coordinates can
   shift between operations. Do NOT reuse coordinates from a previous snapshot.
8. After clicking, always re-snapshot to verify the change before the next interaction.

### Known Limitations

| Issue | Cause | Workaround |
|-------|-------|------------|
| `debug_response` may arrive repeatedly | Bridge loop forwards it on each doc change until cleared | Accept first response; ignore duplicates |
| Widget tree `find_within` fails for splash content | Splash View has parent = -1 in graph | Use coordinates from dump/snapshot for clicks; type_text walks children directly |
| Widget text may show as `" "` (space) instead of `""` | AgentSplash's `__pi_response` label initializes with space | Use `value` field for TextInput, not `text` field |
| Multiple queued clicks may stack before processing | Clicks stored in `pending_click` field | Add delays between click commands, re-snapshot between interactions |
| Click must dispatch directly to splash, not Root | Splash content orphaned (parent=-1) | `splash.handle_event()` not `self.ui.handle_event()` |
| Synthetic events need `WindowId(0,0)` | First window gets index 0 | Use `WindowId(0, 0)` for MouseDown/MouseUp events |
| `text_input.text()` can't read Rust-set values | Splash VM reads from own cache | Use counters and `set_text()` in Splash code instead |
| Debug commands timeout after heavy session use | makepad-host accumulates runtime state that eventually blocks Signal-driven doc sync | **Rebuild + restart**: `cargo build -p harness -p makepad-host` then kill old processes. Or: close app, wait, re-launch. If that fails, restart the harness by killing both processes. |
| RadioButton/ToggleFlat variables don't persist | RadioButton/ToggleFlat internal post-processing loses Splash VM variables set in `on_click` | Use `CheckBox`/`CheckBoxFlat` instead for togglable state variables |
| Window coordinates can shift between snapshots | Makepad window may resize/reposition; orphaned widget coordinates are relative to splash container | Always take a fresh `widget_snapshot` before each click to get current coordinates |
| Stale content after rapid close+launch | If apps are launched quickly, new splash body may not fully replace old one; status line shows old app_id | Close, then wait 1-2 seconds before re-launching |
| TabBar renders with zero size | TabBar widget exists in code but doesn't produce visible output in this build | Don't use TabBar/Tab — use ButtonFlat rows for tab-like UI |
| Slider `on_change` can't be triggered synthetically | Synthetic clicks are MouseDown/MouseUp only; Slider requires mouse drag events | Set slider value via initial `value:` parameter; use ButtonFlat for interactive controls |
| DropDown popup can't be tested with synthetic clicks | Click dispatches to splash widget directly; popup menus are separate overlay windows | Avoid DropDown in testable apps; use ButtonFlat rows for options instead |

### Runtime State Accumulation (Debug Freeze)

**Symptom:** After running multiple apps in rapid succession with many click/snapshot commands, `widget_snapshot` or other debug commands start returning `"No result provided"` or timing out. Even after closing the app and launching a new one, the issue persists.

**Root cause:** The makepad-host process accumulates runtime state across successive app launches and debug operations. Eventually, the Signal-driven event cycle that processes `debug_command` from the shared doc stops responding. The harness bridge loop detects doc changes and forwards messages to pi, but makepad-host never writes `debug_response` back.

**Resolution:**
1. Kill the old processes: `kill <harness_pid> <makepad_host_pid>`
2. Rebuild: `cargo build -p harness -p makepad-host`
3. Launch a new app (this spawns fresh processes)

**Why rebuild helps:** Recompilation ensures the binary matches the current pi extension code. But the more important fix is **killing the old processes** — fresh makepad-host and harness processes start clean.

**Prevention:** If you notice debug commands getting slower or failing, restart early rather than continuing a long test session.

### RadioButton `on_click` Limitation

**Symptom:** A `RadioButton`'s `on_click` callback sets a variable (`selected = "A"`), and the widget's visual `checked` state updates correctly in the widget tree. However, the splash VM variable does NOT persist — reading the variable from a separate Button or Submit handler returns the initial value, not the value set in the `on_click`.

**Verified (2026-06-17):** A RadioButton group was tested where:
- Clicking "Option B" → `on_click` sets `selected = "B"` → widget tree shows `checked: true` ✅
- Clicking "Submit" button → reads `selected` → returns `"None"` (initial value) ❌
- Clicking "Refresh Display" button → calls `set_text()` with `selected` → shows "Selected: None" ❌

**Root cause:** The `RadioButton` widget's internal event handling completes after the user's `on_click` callback runs. During this post-processing, the RadioButton redraws itself and Splash VM variables set during the callback are lost. The same behavior applies to `ToggleFlat`.

**Important:** Even the previously-documented workaround (set variable in `on_click`, read with separate Button) does NOT work — the variable does not persist at all.

**Recommendation:** Use `ButtonFlat` with a manual toggle pattern instead. Neither `RadioButton`/`ToggleFlat` nor `CheckBox`/`CheckBoxFlat` reliably persist Splash VM variables set in `on_click` — only the visual widget-tree state updates correctly.

```splash
// ❌ RadioButton — variable does NOT persist
RadioButton{text:"A" group:1 on_click:||{selected = "A"}}
Button{text:"Submit" on_click:||{ui.__pi_response.set_text(selected)}}  // returns "" (not "A")

// ❌ CheckBox — variable also does NOT persist (confirmed 2026-06-17)
CheckBoxFlat{checked:false on_click:||{toggled = !toggled}}
Button{text:"Submit" on_click:||{ui.__pi_response.set_text("" + toggled)}}  // returns "false" despite visual check

// ✅ ButtonFlat — variable persists correctly
let toggled = false
ButtonFlat{text:"Toggle" on_click:||{toggled = !toggled; ui.display.set_text("" + toggled)}}
ButtonFlat{text:"Submit" on_click:||{ui.__pi_response.set_text("" + toggled)}}  // returns "true"
```

## Debug System Failure Analysis (2026-06-17)

This section catalogs every observed failure mode of the debug system, its root cause
in the code, and the required fix. Based on testing 7 apps across an entire session.

### Failure 1: `wait_for_response` Times Out Despite Doc Having Data

**Observed:** Counter app (counter-1). Click "Send Response" → `__pi_response` label
shows `"counter: 1"` in snapshot → `inspect_makepad_doc` confirms `user_response:
"counter: 1"` → `wait_for_response` times out.

**Root cause:** Two independent data paths:

```
Path A (inspect_makepad_doc — WORKS):
  pi → JSON WS {"type": "get_doc"}
    → harness reads CRDT doc synchronously
    → harness responds {"type": "doc_state", user_response: "..."}
    → pi extension receives response

Path B (wait_for_response — BROKEN):
  splash sets __pi_response → AgentSplash detects change
    → write_doc_field("user_response", data)    // writes to CRDT doc
    → samod WS syncs to harness process           // async hop #1
    → harness bridge loop poll detects change     // async hop #2
    → harness sends {"type": "user_response"}     // JSON WS message
    → pi extension doc-bridge.ts buffers it
    → wait_for_response checks buffer
```

`wait_for_response` relies on the harness bridge loop **asynchronously pushing**
a `user_response` message over JSON WS. If the bridge loop hasn't polled the CRDT
doc changes yet, or if the samod sync is delayed, the message never arrives.
`inspect_makepad_doc` works because it sends a **synchronous query** that reads the
doc directly.

**Code location:**
- Harness bridge loop: `harness/src/main.rs` — polls `doc_handle.changes()`
- wait_for_response: `.pi/extensions/makepad/tools.ts` — reads event buffer only
- inspect_makepad_doc: `.pi/extensions/makepad/tools.ts` — sends `get_doc` query

**Fix:** Make `wait_for_response` also poll the doc directly (like `inspect_makepad_doc`)
if no event arrives within a short timeout. Poll every 500ms up to the full timeout.

```typescript
// Proposed fix for wait_for_response:
async function waitForResponse(appId, timeout) {
  // First check buffer immediately
  const buffered = getBufferedEvent('user_response');
  if (buffered) return { source: 'buffered', ...buffered };
  
  // Then poll doc directly every 500ms
  const deadline = Date.now() + timeout * 1000;
  while (Date.now() < deadline) {
    const doc = await sendQuery({ type: 'get_doc' });
    if (doc.user_response) return { source: 'polled', ...doc };
    await sleep(500);
  }
  throw new TimeoutError();
}
```

---

### Failure 2: Rapid Sequential Clicks Lose Second Click

**Observed:** Todo app (todo-demo-1). Click Task 1 checkbox → click Task 2 checkbox
immediately after → only Task 1 checked. The second click at `(56, 218)` was lost.

**Root cause:** `pending_click` is a single `Option<(f64, f64)>` field:

```rust
// makepad-host/src/app.rs, line 79
pending_click: Option<(f64, f64)>,
```

When two `debug_command=click` messages arrive before a Draw/Signal event processes
them, the second `process_debug_commands()` call **overwrites** the pending click:

```rust
// app.rs, line 265
self.pending_click = Some((x, y));  // Second call: overwrites first click!
```

Then `dispatch_pending_click()` fires **one** click at the second coordinate only.

**Timeline:**
```
Signal 1: process_debug_commands → pending_click = Some((56, 190))  // Task 1
Signal 2: process_debug_commands → pending_click = Some((56, 218))  // OVERWRITES Task 1!
Signal 3: dispatch_pending_click → fires click at (56, 218) only
```

**Fix:** Change `pending_click` from a single option to a `Vec<(f64, f64)>` queue.
Dispatch all pending clicks in order on the next event cycle.

```rust
// Fix:
pending_clicks: Vec<(f64, f64)>,  // queue instead of single slot

fn process_debug_commands(&mut self, cx: &mut Cx) {
    // ...
    self.pending_clicks.push((x, y));  // append to queue
}

fn dispatch_pending_clicks(&mut self, cx: &mut Cx) {
    let clicks = std::mem::take(&mut self.pending_clicks);
    for (x, y) in clicks {
        // dispatch MouseDown + MouseUp at (x, y)
    }
}
```

**Alternative workaround (no code change needed):** Take a fresh snapshot between
each click interaction. Each `widget_snapshot` triggers a Signal cycle, giving the
pending click time to dispatch before the next one is queued.

---

### Failure 3: Stale Content After Rapid Close+Launch

**Observed:** Tabs/dropdown app → close → immediately launch calculator-1 →
widget_snapshot still shows tabs content. Status line says `"App: tabs-dropdown-1"`.

**Root cause:** The deferred update architecture delays splash body evaluation to
the next Draw event:

```rust
// app.rs handle_event():
fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
    if matches!(event, Event::Draw(_)) {
        self.apply_pending_updates(cx);  // ← splash body eval happens HERE
    }
    match event {
        Event::Signal => {
            self.sync_from_doc(cx);       // ← stores PendingUiUpdate
            self.apply_pending_updates(cx);    // also runs here for close/clear
        }
        _ => {}
    }
}
```

**Timeline of rapid close+launch:**
```
Signal A: sync_from_doc → close: PendingUiUpdate { splash_body: "" }
           apply_pending_updates → evaluates empty body ✅
Signal B: sync_from_doc → launch calculator: PendingUiUpdate { body: "calculator..." }
           apply_pending_updates → evaluates calculator body ✅
```

But between Signal A and Signal B, there's a **race window**: if the harness sends
both the `clear` and the `launch` commands in rapid succession, the CRDT doc may
update both fields before makepad-host processes Signal A. In that case,
`sync_from_doc` sees the final state (calculator body) and stores ONE pending
update. But `apply_pending_updates` might not run until the next Draw, and a
`widget_snapshot` taken during this window returns stale content.

**Fix:** In `sync_from_doc`, if a new splash_body is received, force an immediate
`redraw()` to ensure `apply_pending_updates` runs before any intervening snapshot.

```rust
fn sync_from_doc(&mut self, cx: &mut Cx) {
    // ...
    if splash_body != self.last_splash_body || app_id != self.last_app_id {
        self.pending_update = Some(update);
        cx.redraw();  // Force immediate redraw
    }
    // ...
}
```

**Workaround:** Wait 2-3 seconds between `close` and `launch` calls.

---

### Failure 4: RadioButton/ToggleFlat Variables Don't Persist (Deep Dive)

**Observed:** RadioButton clicked → widget tree shows `checked: true` → but splash
VM variable `selected` stays at its initial value when read from any other callback.

**Root cause — code level:**

The Splash VM evaluates widget trees in a sandboxed scope. When a widget declares
an `on_click` closure:

```splash
RadioButton{text:"Option B" group:1 on_click:||{selected = "B"}}
```

The VM creates a **temporary scope** for this closure, executes it, then discards
it. For `CheckBox`, `ButtonFlat`, and other simple widgets, the variable assignment
propagates to the outer scope because those widgets don't have post-processing that
overwrites the scope.

For `RadioButton` (and `ToggleFlat`), after the user's `on_click` closure runs,
the widget's **internal `handle_event`** continues:

```rust
// Pseudocode of RadioButton::handle_event():
fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
    // ...
    if hit_test_passes {
        // Step 1: Run user's on_click closure (temporary VM scope)
        self.run_on_click(cx);  // selected = "B" — but scope is discarded!
        
        // Step 2: Widget's own state update (separate scope)
        self.toggle_selected(cx);  // Updates checked state in widget tree
        self.redraw(cx);
        
        // Step 3: Splash VM scope from Step 1 is now gone
        //    Outer scope never received the variable update
    }
    // ...
}
```

**Why CheckBox works:** `CheckBox`/`CheckBoxFlat` don't have post-processing that
resets the VM scope. The variable set during `on_click` propagates correctly.

**There is no workaround within the RadioButton widget.** Not even using a
separate Button to read the variable works, because the variable was never written
to the outer scope in the first place.

**Fix:** This is a Makepad Splash VM/widget design issue. Would require either:
1. Changing RadioButton's internal event handling to preserve VM scope, OR
2. Having the Splash VM use a persistent scope for closures

---

### Failure 5: Window Coordinates Are Relative, Not Absolute

**Observed:** Orphaned widget coordinates in `widget_snapshot` (those with
`window_id: ""`) show small values like `x=42, y=243`. These are NOT absolute
window coordinates — they are **relative to the splash content view's origin**.

**Why clicks still work:** `dispatch_pending_click` dispatches MouseDown+MouseUp
directly to `splash.handle_event()`:

```rust
// app.rs line 378
let abs = dvec2(x, y);  // Uses orphaned widget coordinates AS-IS
// ...
splash.handle_event(cx, &md_event, &mut Scope::empty());
```

The splash widget receives the event in its **local coordinate space**, which is
the same space the orphaned widgets' coordinates are in. So using the orphaned
coordinates directly works — but they are NOT window-absolute.

**When this breaks:** If the window resizes or repositions between snapshots
(e.g., caption bar hidden/fullscreen toggle), the orphaned widget coordinates
can shift. This happened during the todo app test where the window went from
`x=245, y=144` to `x=0, y=0` between launches — the orphaned widget positions
shifted slightly.

**Fix (documentation only — this is by design):** Always take a fresh
`widget_snapshot` before each click. The coordinates from one snapshot are
only valid within that snapshot's layout. Never cache or reuse coordinates.

```
✅ CORRECT: snapshot → read coords → click → snapshot → read coords → click
❌ WRONG:   snapshot → read coords → click → click (reuses old coords)
```

---

### Failure 6: Debug Commands Freeze After Heavy Use

**Observed:** After many app launches + click/snapshot cycles, `widget_snapshot`
starts returning `"No result provided"` or timing out. Even closing and re-launching
doesn't help.

**Root cause:** The makepad-host process accumulates runtime state across
successive `eval_body()` calls. Each new splash body creates a new VM context,
new widget tree, and new event handlers. Eventually, the Signal-driven event cycle
that reads `debug_command` from the CRDT doc and writes `debug_response` back
stops responding — the process loops internally but never processes new commands.

**Code indicators:**
- Harness bridge loop detects doc changes (keeps forwarding messages to pi)
- makepad-host receives Signal but never writes `debug_response` back to doc
- No crash, no error — just silent non-response

**Resolution:**
1. Kill both processes: `pkill -f makepad-host; pkill -f harness`
2. Rebuild: `cargo build -p harness -p makepad-host`
3. Start fresh — relaunch the app

**Prevention:** Restart the harness after 5-7 app launches, or after ~50 debug
commands. This is a known resource leak in the Splash VM eval cycle.

---

### Summary: What to Fix and Priority

| Priority | Issue | Fix Location | Complexity |
|----------|-------|-------------|------------|
| 🔴 High | `wait_for_response` times out | `tools.ts` — add doc polling fallback | Low (add poll loop) |
| 🔴 High | Rapid clicks overwrite each other | `app.rs` — change to Vec queue | Low (replace Option with Vec) |
| 🟡 Medium | Stale content after rapid close+launch | `app.rs` — force redraw after new body | Medium |
| 🟢 Low | RadioButton/ToggleFlat variable loss | Makepad widget code (upstream) | Very High (Splash VM) |
| 🟢 Low | Window coordinate drift | Documentation only | None |
| 🟢 Low | Debug freeze after heavy use | Runtime state accumulation | High (resource leak) |

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
| `inspect_makepad_doc` queries doc state | ✅ Works — returns app_id, user_response, error, status |
| `wait_for_response` blocks on event | ✅ Timer-based; may timeout if click misses target |
| `type_text` fills only first TextInput | ✅ Fixed — `walk_widgets_set_text` stops after first match |
| Colons inside string arguments to `set_text()` | ✅ Works — `"Time: 2:30"` renders correctly, `"1:00"` sends correctly |
| Splash VM reads Rust-set TextInput values | ✅ Proven: `"A:" + ui.field_a.text()` = `"A:HelloWorld"` after `type_text` |
| Idle CPU (no debug commands) | ✅ 1.7% — no more 100% spin loop from idle Signals |
| Counter app: increment button + display | ✅ Click "+ Increment" → label updates "Count: 1" |
| Text echo: type_text → Show Text → display | ✅ type_text "Hello" → click "Show" → "You typed: Hello Makepad" |
| CheckBox toggle + variable persistence | ⚠️ Visual `checked: true` updates, but Splash VM variable does NOT persist (same issue as RadioButton) |
| RadioButton: visual check but variable lost | ⚠️ `checked: true` in widget tree, but `selected` variable stays "None" |
| Calculator: sequential digit input via buttons | ✅ Click 7 → click 2 → display shows "72" |
| ToggleFlat: variable lost after toggle | ⚠️ Visual state renders but toggled variable doesn't persist |
| Slider renders at correct position | ✅ Visible in snapshot with correct x,y,w,h |
| TabBar: zero-size rendering | ⚠️ width=0, height=0 in widget tree |
| DropDown renders at correct position | ✅ Visible with correct dimensions |
| 7-app succession test: close/launch lifecycle | ✅ All 7 apps launched, tested, and closed successfully |
| Stale content after rapid close+launch | ⚠️ Sometimes old body lingers; fixed by waiting 1-2s |

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
- **`for` loops render widgets at build-time only** — array changes do NOT trigger re-renders; use `set_text()` for dynamic content
- **Functions with `for` loops return empty strings** when called from `on_click` — build strings inline in the closure instead
- **Inline string building with `text()` + `set_text()` + `\n`** is the correct pattern for dynamic list displays

### Mistake: Omitting `height: Fit` on containers

Every `View`, `RoundedView`, etc. **MUST have explicit `height: Fit`**. Without it, the container collapses to zero height and nothing visible renders inside it. This was the single most common cause of "blue rectangle with no content" during testing.

```splash
✅ RoundedView{width:Fill height:Fit flow:Down padding:16 new_batch:true ...}
❌ RoundedView{width:Fill flow:Down padding:16 ...}  ← invisible!
❌ View{padding:30 ...}  ← invisible!
```

Note that `RoundedView` does NOT have a default height of `Fit`. Every container needs it explicitly.

### Mistake: Relying on `for` loops for dynamic rendering or string building

**`for` loops in Splash DSL only render widgets at build time, not dynamically.**

When a `for` loop generates widgets (like a task list), those widgets are created
once when the body is first evaluated and **never re-rendered** when the underlying
array changes. This means:

```splash
// ❌ WON'T WORK — for loop creates widgets once, never updates
let tasks = []
for t in tasks {
  Label{text: t}
}
ButtonFlat{text:"Add" on_click:||{
  tasks = tasks + ["New task"]  // Array updates, but UI doesn't re-render!
}}
```

**Functions with `for` loops that build strings also return empty results** when
called from `on_click` handlers. The string accumulation logic (`result = result + t`)
inside a function called from a closure produces empty strings:

```splash
// ❌ WON'T WORK — function with for loop returns empty string
fn task_display_text() {
  let result = ""
  for t in tasks {
    result = result + t
  }
  result
}
ButtonFlat{text:"Show" on_click:||{
  ui.display.set_text(task_display_text())  // Shows nothing!
}}
```

**✅ Correct pattern: Inline string building with `text()` + `set_text()`**

Instead of relying on `for` loops or functions, build the display string inline
in the `on_click` handler by reading the current label text and appending:

```splash
let task_count = 0
RoundedView{width:Fill height:Fit flow:Down padding:16 spacing:8
  task_input := TextInput{width:200 height:34}
  ButtonFlat{text:"Add" on_click:||{
    let t = ui.task_input.text()
    if t != "" {
      task_count = task_count + 1
      let current = ui.task_display.text()
      if current == " " { current = "" }         // Handle initial space
      if current != "" { current = current + "\n" }  // Newline separator
      ui.task_display.set_text(current + task_count + ". " + t)
      ui.task_input.set_text("")
    }
  }}
  task_display := Label{text:"" font_size:14.0}
}
```

There is **no workaround** for dynamic widget generation via `for` loops in Splash
DSL — you cannot re-render a subtree when an array changes. Always use string-based
`set_text()` for dynamic content.

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
| App launched but status stuck on "Pending" | Pi extension resolved on first status (Pending) before makepad-host updated to Launched; this is a known race condition |
| Click misses target / wrong widget hit | Window coordinates shifted between snapshot and click; orphaned coordinates are relative to splash container, not absolute window | Always take fresh `widget_snapshot` before clicking; recalculate center each time |
| Stale old content shows after launching new app | Rapid close+launch; makepad-host hasn't processed the new splash body yet | Close app, wait 1-2 seconds, then launch new app |
| CheckBox/RadioButton/ToggleFlat shows checked but variable is initial value | Widget internal post-processing loses `on_click` variable assignments for CheckBox, RadioButton, and ToggleFlat | Use `ButtonFlat` with manual toggle instead |
| Debug commands timing out after many interactions | Runtime state accumulation in makepad-host after many app launches | Kill both processes, rebuild, and restart

## Widget Reliability Reference (2026-06-17)

Comprehensive reference based on testing 7 apps with 25+ widget instances. Use this table to determine
which widgets to use for reliable interactive apps.

### Fully Reliable Widgets

| Widget | Verified Capabilities | Best For |
|--------|----------------------|----------|
| **`ButtonFlat`** | Click → variable write, `set_text()`, `text()`, `__pi_response.set_text()` | All interactive controls — input, submit, toggle |
| **`Button`** | Same as ButtonFlat | Standard buttons |
| **`Label`** | `set_text()` updates visible text, inline expressions work at build time | Display values, titles, status |
| **`TextInput`** | `type_text` fills first input, `text()` reads value, `set_text()` writes | Text entry, editable fields |
| **`Hr`** | Renders full-width line divider | Visual separation between sections |
| **`RoundedView`** | Container with rounded corners, groups child widgets correctly | App root container, grouping |

### Reliable Patterns

**Sequential digit input (calculator-style):**
```splash
let a = 0
ButtonFlat{text:"7" on_click:||{a = a*10+7; ui.display.set_text("" + a)}}
```
Chain: click "7" → a=7, click "2" → a=72. Verified working.

**Counting with let variable reassignment:**
```splash
let count = 0
let done = 0
if task_done { done = done + 1 }  // Works in Splash DSL
ButtonFlat{text:"Submit" on_click:||{ui.__pi_response.set_text("" + count)}}
```

**Inline string building for dynamic lists (todo-style):**
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
  }
}}
```
This pattern replaces broken `for`-loop-based rendering. Append to the label's
text on each add operation using `ui.<name>.text()` to read current content,
then `ui.<name>.set_text()` to write the updated content. Works with
multiline `\n` content.

**type_text → click pipeline:**
```splash
field := TextInput{width:Fill height:34}
ButtonFlat{text:"Show" on_click:||{ui.display.set_text(ui.field.text())}}
```
1. `check_debug_app debug_command=type_text debug_params="Hello"`
2. `check_debug_app debug_command=click debug_params='{"x":...,"y":...}'`
3. `check_debug_app debug_command=widget_snapshot debug_params="{}"`

### Available But Not Interactive via Synthetic Clicks

| Widget | Verified | Limitation |
|--------|----------|------------|
| **`Slider`** | Renders, visible at correct position | `on_change` needs mouse drag — can't trigger with synthetic click |
| **`DropDown`** | Renders at correct position | Popup menu is a separate overlay window — can't click menu items synthetically |

### Visual-Only Widgets (Variables Don't Persist)

| Widget | Visual State | Variable Persistence |
|--------|-------------|---------------------|
| **`RadioButton`** | `checked: true` updates correctly in widget tree | ❌ Splash VM variable set in `on_click` is LOST after RadioButton internal post-processing |
| **`ToggleFlat`** | `checked` visual state renders | ❌ Same limitation as RadioButton — variable doesn't persist |
| **`CheckBox`** / **`CheckBoxFlat`** | `checked: true` updates correctly in widget tree | ❌ Same limitation as RadioButton — variable doesn't persist (confirmed 2026-06-17) |

**Recommendation:** Use `ButtonFlat` with manual toggle pattern for persistent state variables.

### Unavailable / Non-Rendering

| Widget | Observed Behavior |
|--------|------------------|
| **`TabBar`** / **`Tab`** | Present in widget tree with width=0, height=0 — no visible output |
| **`Stack`** | Not in widget source files in this build |
| **`Divider`** | Not in widget source files; use `Hr` instead |
| **`ProgressBar`** | Not in widget source files; use read-only `Slider` |
| **`IconButton`** | Not in widget source files |
| **`ToggleButton`** | Not in widget source files; use `ToggleFlat` (visual only) |
| **`Image`**, **`ListView`**, **`Grid`** | Not in widget source files |
| **`ColorPicker`**, **`ScrollPair`** | Not in widget source files |

### Best Practices for Testable Apps

1. **Use `ButtonFlat` as your primary interactive widget** — most reliable for clicks, variable writes, and `__pi_response`
2. **Avoid `CheckBoxFlat`, `RadioButton`, and `ToggleFlat`** for persistent state variables — only visual `checked` state toggles
3. **Use `ButtonFlat` with manual toggle** for boolean state that needs to persist
4. **Use `Hr` for dividers** instead of unavailable `Divider`
5. **Always take a fresh `widget_snapshot`** before clicking — coordinates can shift
6. **Wait between interactions** — rapid sequential clicks may stack
7. **Use `close` + wait 1-2s + `launch`** if a new app shows stale content
8. **Do NOT use `for` loops for dynamic content** — `for` loops only render at build time; use `set_text()` string building instead
9. **Do NOT call functions containing `for` loops from `on_click`** — they return empty strings; inline all string building in the closure
10. **Use `text()` + `set_text()` with `\n` concatenation** for multiline list displays in a single Label

### Logs

The harness and makepad-host both output debug info via `eprintln!` to stderr:
- Harness runs in the background, stderr goes to the `pi` terminal
- makepad-host is spawned with `Stdio::inherit()` for stderr, so its logs also go to the `pi` terminal
- `[harness]`, `[makepad-host]`, `[splash]` prefixes identify the source

If you can't see logs, check if the pi process is running in a visible terminal.

## Interactivity Test Results (verified 2026-06-17)

### Within-App Interactivity

| Test | Result | Notes |
|------|--------|-------|
| `ButtonFlat` clicks (`on_click`) | ✅ Works | State vars update, UI reflects changes — most reliable widget |
| `ui.<name>.set_text()` | ✅ Works | Updates any widget's text correctly |
| `ui.<name>.text()` | ✅ Works | Reads TextInput content from Splash VM |
| Multiple statements in closure | ✅ Works | Use `;` separator inside `{ }` |
| Functions (`fn foo(){...}`) | ✅ Works | Can call `ui.*` and functions |
| `set_interval()` / `clear_interval()` | ❌ NOT available | Not in Makepad script VM |
| `send_response()` from splash body | ❌ Not callable | Only callable from parent app code; use `__pi_response.set_text()` instead |
| App launch/replacement | ✅ Works | New `launch` replaces old app; close+re-launch fixes stale state |
| Conditional `if` rendering | ✅ Works | Works at widget level |
| `as int` type casting | ❌ Produces NaN | `val as int` on a string value gives `NaN`; use string display + `set_text()` instead |
| Inline variable in Label text | ⚠️ Static only | `Label{text:"Count: " + count}` evaluated at build time; to update, use `ui.<name>.set_text()` |
| `let variable reassignment` | ✅ Works | `let x = 0; x = x + 1` works for counters and accumulators |
| `CheckBox` / `CheckBoxFlat` toggle | ⚠️ Visual only | Visual `checked` state updates, but Splash VM variable does NOT persist; use ButtonFlat for persistent state |
| `RadioButton` group selection | ⚠️ Visual only | `group:1` parameter enables visual radio group; `checked` state renders correctly BUT splash VM variable set in `on_click` does NOT persist (see RadioButton limitation) |
| `ToggleFlat` toggle | ⚠️ Visual only | Same limitation as RadioButton — visual state renders but variable doesn't persist |
| `type_text` + button click pipeline | ✅ Works | type_text fills first TextInput; button click reads value via `ui.<name>.text()` correctly |
| `Hr{height:1 width:Fill}` divider | ✅ Works | Renders a visible horizontal rule |
| `Slider` widget renders | ✅ Renders | Present and visible, but `on_change` can't be triggered via synthetic click (needs mouse drag) |
| `TabBar` + `Tab` | ❌ No visible output | TabBar renders with width=0, height=0 in widget tree — no visible tabs |
| `DropDown` + `PopupMenu` | ✅ Renders | DropDown is visible at the correct position, but popup menu items can't be tested with synthetic click |
| `send_response()` via `__pi_response.set_text()` | ✅ Works | Hidden label writes response to shared doc, forwarded by harness bridge |
| Deferred UI updates | ✅ Works | sync_from_doc on Signal → store pending → apply on Draw |
| Synthetic click dispatch to splash | ✅ Works | Dispatch directly to AgentSplash (not through Root/Window) |
| Close app clears visual state | ✅ Works | Empty splash body renders empty View |
| Sequential digit input via buttons | ✅ Works | Pattern `a = a*10+7` builds multi-digit numbers from button clicks |
| `for` loop widget rendering | ❌ Static only | Widgets from `for` loops are created at build time; array changes don't re-render — use `set_text()` for dynamic content |
| `for` loop string building in functions | ❌ Returns empty | Functions containing `for` loops called from `on_click` return empty strings — use inline string building instead |
| Inline `\n` string building in `on_click` | ✅ Works | `ui.<name>.set_text(current + "\n" + new_item)` builds multiline displays dynamically — verified with 3-item task list |
| Calculator arithmetic (72 + 19) | ✅ Works | `a = a*10+digit` pattern + `b = a; a = 0` for operand swap; `if op == "+" { a = a + b }` computes addition |

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

## CRDT State: In-Memory Only, No Disk Persistence

The CRDT document used to communicate between the harness and makepad-host is **purely in-memory**.

- `samod::Repo::build_tokio()` returns `RepoBuilder<InMemoryStorage, ...>` — samod's default storage backend is in-memory
- `repo.create(initial)` creates a fresh document each time — no file backing
- `automerge::Automerge::new()` creates a fresh document in memory — no file persistence
- The only files written are **ready markers** (e.g., `/tmp/makepad_host_ready_*`) — simple text files to signal process readiness, not CRDT data
- When the harness exits, all CRDT state is **gone permanently**
- When a new harness starts, it creates a fresh doc with a default `AgentDoc` (all fields cleared)

**What this means:**
- No stale state can leak between sessions
- Restarting the harness (killing both processes) always starts clean
- No files to clean up (except ready markers, which are auto-removed)

## End of task flow

- At the end of a task, suggest a commit message to the user, based on the current diff.