# A2App Harness — Architecture & State

## 1. Architecture Overview

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

### Components

#### 1. Rust Harness (`harness/src/main.rs`)

Background tokio process. Creates a samod repo with a single shared CRDT document (`AgentDoc`), runs JSON WS server on :2341 and samod WS server on :2342, spawns makepad-host as child, and bridges messages between pi and the CRDT doc.

Env: `HARNESS_HEADLESS=1` — skip spawning makepad-host (for testing).

#### 2. Makepad Host (`makepad-host/src/main.rs`)

The Makepad UI process. Connects to harness samod WS, finds the shared document, renders splash in `AgentSplash` widget. Background thread listens for doc changes → signals main thread via `SIGUSR1`.

AgentSplash injects three hidden widgets into every splash body:
- `__pi_response := Label{text:""}` — apps call `set_text()` to send data back to pi
- `__pi_data := Label{text:" "}` — apps read `text()` to receive data from pi
- `__ai_text := TextInput{height:34 width:Fill}` — auto-displays AI responses from sub-agent sessions

Env vars (set by harness): `MAKEPAD_HOST_DOC_ID`, `MAKEPAD_HOST_WS_URL`, `MAKEPAD_HOST_READY_MARKER`.

#### 3. Pi Extension (`.pi/extensions/makepad/`)

TypeScript extension. Key files:
- `tools.ts` — `launch_makepad_app`, `close_makepad_app`, `list_makepad_apps`, `check_debug_app`, `inspect_makepad_doc`, `wait_for_response`
- `doc-bridge.ts` — WebSocket client, event buffer
- `harness.ts` — spawns/manages the harness binary
- `validate-splash.ts` — splash body pre-validation

Both `validate-splash.ts`/`dist/validate-splash.js`, `harness.ts`/`dist/harness.js`, and `tools.ts`/`dist/tools.js` must be kept in sync — pi loads from `dist/`.

### JSON WS Protocol (pi ↔ harness, port 2341)

#### Pi → Harness
```json
{"type": "launch", "app_id": "todo-1", "splash_body": "..."}
{"type": "clear", "app_id": "todo-1"}
{"type": "debug", "app_id": "todo-1", "command": "widget_snapshot", "params": "{}"}
{"type": "send_pi_response", "app_id": "todo-1", "data": "..."}
{"type": "get_doc"}
{"type": "exit"}
```

#### Harness → Pi
```json
{"type": "welcome"}
{"type": "status", "app_id": "todo-1", "status": "Launched"}
{"type": "user_response", "app_id": "todo-1", "response": "..."}
{"type": "debug_response", "app_id": "todo-1", "result": "..."}
{"type": "error", "app_id": "todo-1", "message": "..."}
{"type": "doc_state", "app_id": "todo-1", "user_response": "...", "error_message": "...", "status": "...", "pi_response": "..."}
```

### Communication Flows

#### Launch App
1. pi sends `{"type":"launch","app_id":"...","splash_body":"..."}` over JSON WS
2. Harness writes `pending_app` to CRDT doc (Pending → Launched)
3. CRDT syncs to makepad-host over samod WS
4. Makepad-host renders splash in AgentSplash widget on next Draw event

#### User Response (splash → pi)
1. Splash app calls `ui.__pi_response.set_text("data")` in any `on_click` handler
2. AgentSplash detects the label text changed → writes `user_response` to CRDT doc
3. AgentSplash also increments `user_response_version` before writing
4. Harness bridge loop compares version number (not value) to detect changes
5. Harness forwards `{"type":"user_response","app_id":"...","response":"..."}` to pi
6. Pi extension buffers the event (per-type Map) and dispatches to `wait_for_response`

#### Pi Response (pi → splash)
1. pi (or extension auto-handler) sends `{"type":"send_pi_response","app_id":"...","data":"..."}` over JSON WS
2. Harness writes `pi_response` to CRDT doc + sets `extension_requests = true`
3. CRDT syncs to makepad-host over samod WS
4. Background thread detects `pi_response` change → signals UI thread
5. AgentSplash reads `pi_response`, writes it to `__ai_text` widget (TextInput) and `__pi_data` label
6. Splash app reads response via `ui.__ai_text.text()` or `ui.__pi_data.text()`

#### Shutdown
1. pi sends `{"type":"exit"}` or pi exits
2. Harness sets `should_exit = true` in the doc
3. Harness kills makepad-host child process and exits

### Shared Document (`AgentDoc` in `shared/src/lib.rs`)

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
    pub pi_response: Option<String>,       // pi sends data to splash
}
```

CRDT is in-memory only — no disk persistence. Restarting always starts clean.

---

## 2. Debug System (`check_debug_app`)

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
- `type_text` walks `try_children()` on the splash WidgetRef (which delegates to AgentSplash's inner View's children list — the outer View from SPLASH_PREFIX)
- **Always use coordinates from the dump/snapshot for clicks** — widget_id lookups via `find_within` fail

### Coordinate System (CRITICAL)

**Orphan widget coordinates in `widget_dump` and `widget_snapshot` are WINDOW-RELATIVE** — they are relative to the window's content area top-left (0,0 at window's content origin, NOT including the window's screen position).

This was confirmed by testing (2026-06-24):
- Window at screen position (245, 144)
- AgentSplash at snapshot absolute (271, 276) → window-relative: (271-245=26, 276-144=132)
- Orphan outer View at window-relative (26, 132) — MATCHES AgentSplash's window-relative position
- Orphan "-" button at window-relative (447, 135) → clicked at center (457, 146) → COUNTER DECREMENTED ✅

**To click: use orphan coordinates directly — they are already in window-relative space.**

```
click_x = orphan_widget.x + (orphan_widget.width / 2)
click_y = orphan_widget.y + (orphan_widget.height / 2)
```

Do NOT add window position. Do NOT add parent offsets. The orphan coordinates in the dump/snapshot are absolute window-relative positions.

**Example from the compact_dump format (W3):**
```
# Parent ID is shown but coordinates are WINDOW-RELATIVE, not parent-relative
9 -1 - View 26 132 928 105           # orphan View, window-relative (26,132)
10 9 display Label 50 162 22 32      # display at (50,162) in window space
11 9 - Button 442 224 41 22           # Click button at (442,224), center (462,235)
```
Click at window-relative (462, 235) to hit the button at (442, 224, 41, 22).

### Clipped Rect Issue (Critical for Nested Widgets)

Containers with `padding` or `show_bg: true` create draw clips that affect their children's `area.clipped_rect()`. The hit-test for mouse events uses `area.clipped_rect()`, NOT `area.rect()`. If a child widget overflows the parent's padded content area, its `clipped_rect` is reduced to the overlap, potentially making it UNHITTABLE.

**Tested (2026-06-24):**
- **Direct orphans (parent=-1) with no container wrapping**: Buttons hittable ✅
- **Nested inside `View{height:Fit}` without padding**: Buttons hittable ✅
- **Nested inside `RoundedView{padding:16}` where buttons overflowed padded area**: Buttons NOT hittable ❌

**Workaround:** Keep interactive buttons as direct orphans (not wrapped in containers with padding), or ensure they fit within the parent's padded content area.

### First Use Pattern (Standard Interaction Workflow)

1. **Launch**: `launch_makepad_app app_id="my-app" splash_body="..."`
2. **Snapshot**: `check_debug_app debug_command=widget_snapshot debug_params="{}"` — find orphaned widgets at bottom (`"window_id": ""`)
3. **Calculate click center**: orphan widget coordinates ARE window-relative, so use `x + w/2, y + h/2` directly
4. **Click**: `check_debug_app debug_command=click debug_params='{"x":490,"y":185}'`
5. **Verify**: Use `inspect_makepad_doc` to read `user_response` (synchronous, always works) OR `wait_for_response`
6. **For TextInput**: `type_text` fills the first TextInput found in the splash body's widget hierarchy. To verify which input was filled, check the `value` field on orphan TextInputs in `widget_snapshot`.

**CRITICAL: Always take a fresh snapshot before each click** — orphan coordinates shift after layout changes (e.g., adding list items moves buttons down).

**Use `inspect_makepad_doc` for response** — `wait_for_response` may time out if the response arrived before the listener was set up (the listener is event-driven and events may be missed during tool transitions). `inspect_makepad_doc` is synchronous and always reflects the current doc state.

### Known Interaction Issues

**Coordinates shift after layout changes**
When content grows (e.g., items added to a list via `set_text()`), the splash container height changes and all subsequent widgets shift downward. The orphan coordinates from the initial snapshot become stale. **Always take a fresh snapshot before each click** if the UI has changed since the last snapshot.

**`type_text` fills the first TextInput within splash children**
The `type_text` command calls `walk_widgets_set_text(splash, ...)` which walks the AgentSplash widget's child hierarchy via `try_children()` (which delegates to the inner View's children list — the outer View from SPLASH_PREFIX). This means it walks the splash body's widget tree, NOT the main UI tree, so it will **not** accidentally fill the makepad-host `source` editor. It fills the first TextInput found in depth-first order (stops at first match).

**Tested (2026-06-24):** Body with `inp := TextInput{height:34}` as first child → `type_text` filled `inp` with `value: "hello world"` ✅

**Also tested: clicking at coordinates that don't hit any widget (e.g., (5,5)) is a harmless no-op — no crash, no response sent.** ✅

**Also tested: empty string `send_pi_response` is a harmless no-op.** ✅

### Rendering Error Handling

When splash body fails to render:
1. Makepad renders dark-red error fallback ("Splash app could not be rendered")
2. `error_message` is written to CRDT doc
3. Harness forwards `{"type":"error","app_id":"...","message":"..."}` to pi
4. The launch tool has a 1.5s debounce window after receiving `status=Launched` to collect any error messages. Errors persist in a `lastErrors` map per app_id.

---

## 3. Background Sub-Agent Sessions

Splash apps can communicate with background AI sub-agent sessions created via the pi SDK.
The sub-agent is an independent `AgentSession` that processes prompts and returns responses.

### Protocol

Splash sends: `ui.__pi_response.set_text("ai:ask:" + message)`
Splash reads: `ui.__pi_data.text()` (response from sub-agent)

### Auto-Display via `__ai_text`

The AgentSplash injects a `__ai_text := TextInput{height:34 width:Fill}` widget that
auto-displays the sub-agent's response — no manual reading needed. When `pi_response`
is written to the CRDT doc, the background sync thread signals the UI, and the
AgentSplash calls `__ai_text.set_text(response)` automatically.

### Injected Widgets

| Widget ID | Type | Purpose |
|-----------|------|---------|
| `__pi_response` | `Label{text:""}` (hidden) | Splash writes to send responses to pi |
| `__pi_data` | `Label{text:" "}` (hidden) | Splash reads to get data from pi |
| `__ai_text` | `TextInput{height:34 width:Fill}` (visible) | Auto-displays AI response from sub-agent |

### Workflow

1. **Create sub-agent**: `start_background_session(provider="deepseek", model_id="deepseek-v4-flash", system_prompt="...")`
2. **Launch app with session**: `launch_makepad_app(app_id="my-app", splash_body="...", agent_session_id="<sid>")`
3. **User sends message**: splash calls `ui.__pi_response.set_text("ai:ask:" + msg)`
4. **Auto-handler** (extension) detects `user_response` → routes to sub-agent → calls `session.prompt()`
5. **Response sent back**: auto-handler calls `sendToHarness({ type: "send_pi_response", ... })`
6. **Harness writes doc**: `pi_response = "..."` + `extension_requests = true`
7. **Signal fires** → `sync_pi_data_to_splash` reads doc → `__ai_text.set_text(response)`
8. **Response visible** on screen automatically

### Extension Tools

| Tool | Description |
|------|-------------|
| `start_background_session` | Create a sub-agent session. Pass `provider`, `model_id`, `system_prompt`, `thinking_level` |
| `send_background_message` | Send a prompt to an existing sub-agent, wait for response |
| `list_background_sessions` | List all active sub-agent sessions |
| `stop_background_session` | Stop and dispose a sub-agent session |
| `send_pi_response` | Send data from pi to the splash app (read by splash via `ui.__pi_data.text()`) |

### Splash Body Template

Minimal splash body that uses the sub-agent:

```splash
inp := TextInput{width:Fill height:34 empty_text:"Your message..." on_return:|t|{
  if t != "" { ui.__pi_response.set_text("ai:ask:" + t); ui.inp.set_text("") }
}}
ButtonFlat{text:"Send" on_click:||{
  let m = ui.inp.text()
  if m != "" { ui.__pi_response.set_text("ai:ask:" + m); ui.inp.set_text("") }
}}
```

The response auto-appears in the `__ai_text` TextInput at the bottom of the layout.
No display widget needed in the splash body.

---

## 4. Splash DSL Guide (General Reference)

This section covers general Makepad Splash DSL patterns that apply to ANY app body, not just this harness. These are verified to work with the AgentSplash embedded renderer.

### 4.1 Key Rules

- **`let`/`fn` declarations must be at the top**, before any widget. The body starts with declarations, then the root widget.
- **Every container MUST have `height: Fit`** — most common failure mode. Inside a fixed-height parent, `height: Fill` is fine.
- **Root container MUST use `width: Fill`** — never a fixed pixel width. The app renders inside a parent container that provides the width.
- `ui` object is built-in; do NOT declare it with `:=`
- **`for` loops render widgets at build time only** — array changes do NOT re-render. Use `set_text()` for dynamic content.
- **Functions with `for` loops return empty strings** when called from `on_click` — inline string building instead
- **`as int` type casting produces NaN** — use string display + `set_text()` only
- **Colons inside string arguments work correctly** — `"Time: 2:30"` is fine
- Every `TextInput` must have a fixed numeric height (e.g. `34`)
- No `on_render` in embedded apps

### 4.2 Widget Availability

**Available:** View, RoundedView, Label, TextInput, LinkLabel, Button, ButtonFlat, ButtonFlatter, Slider, CheckBox, CheckBoxFlat, RadioButton, RadioButtonFlat, ToggleFlat, DropDown, TabBar, Tab, PopupMenu, ScrollBar, ScrollBars, LoadingSpinner, Hr, Vr, Icon

**NOT available (silently fail):** Stack, Divider, ProgressBar, IconButton, ToggleButton, Image, ListView, Grid, ColorPicker, ScrollPair

| Wanted | Not Available | Use Instead |
|--------|--------------|-------------|
| Divider line | `Divider` | `Hr{height:1 width:Fill}` |
| Progress bar | `ProgressBar` | `Slider{value:0.65 is_read_only:true}` |
| Tabbed UI | `TabBar`/`Tab` | `ButtonFlat` rows (TabBar renders zero-size) |

### 4.3 Styling Gotchas

**`draw_bg.border_radius` takes a float, not an Inset:**
```splash
// ✅
draw_bg.border_radius: 16.0

// ❌ parse error — silently breaks layout
draw_bg.border_radius: Inset{top:0 bottom:16 left:0 right:0}
```

**`#x` prefix for hex colors containing 'e':** When a hex color contains the letter `e` adjacent to digits (like `#1e1e2e`), use `#x` to avoid parser ambiguity. Without `#x`, Makepad's parser may misinterpret digits following 'e' as an exponent:
```splash
#x2ecc71     // ✅ contains 'e' next to digits, use #x
#x1e1e2e     // ✅ contains 'e' next to digits, use #x
#ff4444      // ✅ no 'e' issue, plain # works
#00ff00      // ✅ no 'e' issue
```

**Default text color is white:** All text widgets (`Label`, `Button`, etc.) default to `#fff`. For light/white backgrounds, you MUST explicitly set `draw_text.color` to a dark color on every text element:
```splash
RoundedView{draw_bg.color:#f5f5f5 height:Fit
  Label{text:"Visible!" draw_text.color:#x222}
}
```

**Label styling shorthand:** Both syntaxes work:
```splash
Label{text:"Hello" color:#x2ecc71 font_size:16}              // bare props work
Label{text:"Hello" draw_text.color:#x2ecc71 draw_text.text_style.font_size:16}  // draw_text also works
```

**`new_batch: true` for text visibility:** Required on any container with `show_bg: true` that contains text children. Without it, text renders behind the background (invisible):
```splash
// ✅ Correct
RoundedView{width:Fill height:Fit new_batch:true show_bg:true draw_bg.color:#x334
  Label{text:"Visible" draw_text.color:#fff}
}
// ❌ Text may be invisible (draws behind bg)
RoundedView{width:Fill height:Fit show_bg:true draw_bg.color:#x334
  Label{text:"Invisible!" draw_text.color:#fff}
}
```

### 4.4 Widget Reliability Reference

| Widget | Capabilities | Best For |
|--------|-------------|----------|
| **`ButtonFlat`** | Click → variable write, `set_text()`, `text()`, `__pi_response.set_text()` (harness-specific) | All interactive controls |
| **`Button`** | Same as ButtonFlat | Standard buttons |
| **`Label`** | `set_text()` updates visible text, `text()` reads back | Display values, status, dynamic list display |
| **`TextInput`** | `type_text` fills first input, `text()` reads value, `set_text()` writes | Text entry |
| **`Hr`** | Full-width line divider | Visual separation |
| **`RoundedView`** | Container with rounded corners | App root, groups |

### 4.5 Splash VM Variable Scope

**`let` variables DO persist** across click events in the same app session:
- Counter: `let count = 0; count = count + 1` correctly produces `1, 2, 3, 4` across consecutive clicks
- Toggle: `let toggled = false; toggled = !toggled` persists `true` state across separate button clicks

However, **widget `checked` state** on `RadioButton`, `ToggleFlat`, `CheckBox` does NOT persist because internal post-processing discards the `on_click` scope context.

| Widget | Visual State | Variable Persistence |
|--------|-------------|---------------------|
| **`RadioButton`** | `checked: true` in widget tree | ❌ Lost — internal post-processing discards `on_click` scope |
| **`ToggleFlat`** | `checked` visual renders | ❌ Same limitation |
| **`CheckBox`** / **`CheckBoxFlat`** | `checked: true` in widget tree | ❌ Same limitation |

**Use `ButtonFlat` with manual toggle for persistent boolean state:**
```splash
let toggled = false
ButtonFlat{text:"Toggle" on_click:||{toggled = !toggled; ui.display.set_text("" + toggled)}}
ButtonFlat{text:"Submit" on_click:||{ui.__pi_response.set_text("" + toggled)}}
```

### 4.6 Container Padding Clips Children's Hit Areas (Harness-Specific)

When using this harness's synthetic click system (`check_debug_app` with `debug_command=click`), containers with `padding` (especially `RoundedView{padding:...}` with `show_bg:true`) create a `draw_clip` in their shader. The hit-test uses `area.clipped_rect()` which includes the parent's clip. Buttons that overflow the padded content area become unhittable.

**Workaround:** Keep interactive buttons as direct orphans without container wrapping:
```splash
// ✅ WORKS for synthetic clicks
ButtonFlat{text:"Click" width:Fill height:40 on_click:||{...}}

// ✅ Also works if button fits within padded area
RoundedView{padding:16 height:Fit
  ButtonFlat{text:"Click" width:Fill height:20 on_click:||{...}}
}
```

### 4.7 Patterns

#### 4.7.1 Struct Arrays & Array Operations

The Splash VM supports arrays of structs with `.push()`, `.remove()`, `.len()`, and `.retain()`. Read fields via `array[index].field`, update with `array[index] += {field: val}`:

```splash
let items = [
    {text: "Task 1" tag: "work" done: false}
    {text: "Task 2" tag: "personal" done: false}
]
let max_items = 5

fn add_item(text){
    let clean = ("" + text).trim()
    if clean == "" { return }
    if items.len() >= max_items { return }
    items.push({text: clean tag: "" done: false})
    sync_all()
}

fn toggle_item(index){
    if index >= items.len() { return }
    items[index] += {done: !items[index].done}
    sync_all()
}

fn remove_item(index){
    if index >= items.len() { return }
    items.remove(index)
    sync_all()
}

fn clear_flagged(){
    items.retain(|it| !it.done)
    sync_all()
}
```

#### 4.7.2 Component / Template Pattern

Define reusable templates with `let` and instantiate with property overrides:

```splash
let ItemRow = RoundedView{
    width: Fill height: Fit
    padding: Inset{top: 8 bottom: 8 left: 12 right: 12}
    flow: Right spacing: 10
    align: Align{y: 0.5}
    new_batch: true
    draw_bg.color: #x2a2a3a
    draw_bg.border_radius: 8.0
    label := Label{text: "item" width: Fill draw_text.color: #xddd}
    action := ButtonFlatter{text: "Do" width: 56 height: 28}
    remove := ButtonFlatter{text: "X" width: 56 height: 28}
}

row_0 := ItemRow{
    label.text: "First item"
    action.on_click: || do_something(0)
    remove.on_click: || remove_item(0)
}
```

Override syntax: `<child-name>.<property>: <value>` — every segment in the path must use `:=`.

#### 4.7.3 Pre-allocated Fixed Slots

`for` loops render at build-time only — array changes don't add/remove widgets. Pre-allocate a fixed number of rows and update via sync functions:

```splash
let items = [{text: "Item 1"} {text: "Item 2"}]

fn sync_row_0(){
    if 0 < items.len() {
        ui.row_0.label.set_text(items[0].text)
    } else {
        ui.row_0.label.set_text("Empty slot")
    }
}
fn sync_rows(){
    sync_row_0()
    sync_row_1()
    sync_status()
}
```

Pre-allocate 5 rows for a 5-item max list. Call `sync_rows()` after every mutation.

#### 4.7.4 Numeric State Pattern

```splash
let count = 0
RoundedView{width:Fill height:Fit flow:Down spacing:10 padding:16 new_batch:true
  display := Label{text:"0" draw_text.color:#x44cc88 draw_text.text_style.font_size:32}
  View{flow:Right spacing:12 align:Align{x:0.5 y:0.5}
    ButtonFlat{text:"-" on_click:||{count -= 1; ui.display.set_text(count + "")}}
    ButtonFlat{text:"Reset" on_click:||{count = 0; ui.display.set_text("0")}}
    ButtonFlat{text:"+" on_click:||{count += 1; ui.display.set_text(count + "")}}
  }
}
```

Use `count + ""` to convert numbers to strings.

#### 4.7.5 Dynamic List Display

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

⚠️ Buttons shift down as items are added — always take a fresh snapshot before clicking.

#### 4.7.6 TextInput with on_return

```splash
input := TextInput{
    width: Fill height: 34
    empty_text: "Enter something"
    on_return: |text| add_item(text)
}
Button{text: "Add" width: 64 height: 34 on_click: || add_item(ui.input.text())}
```

#### 4.7.7 Sequential Digit Input

Perform arithmetic by accumulating digits:
```splash
let a = 0
ButtonFlat{text:"7" on_click:||{a = a*10+7; ui.display.set_text("" + a)}}
```

### 4.8 Naming Children: `:=` vs `:`

Use `:=` for addressable children, `:` for static children:
```splash
label := Label{text:"default"}    // ✅ addressable via ui.label, overridable
label: Label{text:"default"}     // ❌ static — NOT addressable
```

Every path segment in an override must use `:=`:
```splash
// ✅ Correct
let Item = View{flow:Right
  texts := View{flow:Down
    label := Label{text:"default"}
  }
}
Item{texts.label.text:"new text"}  // works!

// ❌ Wrong — anonymous parent blocks override
let Item = View{flow:Right
  View{flow:Down
    label := Label{text:"default"}  // UNREACHABLE
  }
}
Item{label.text:"new text"}  // silent failure
```

### 4.9 Styling Reference

| Property | Example | Effect |
|----------|---------|--------|
| `draw_bg.color` | `#x1e1e2e` | Background color (hex) |
| `draw_bg.border_radius` | `10.0` | Rounded corners |
| `draw_text.color` | `#xddd` | Text color |
| `draw_text.text_style.font_size` | `14` | Font size (float) |
| `padding` | `Inset{top:8 bottom:8 left:12 right:12}` | Inner padding |
| `spacing` | `10` | Gap between children in flow |
| `align` | `Align{x:0.5 y:0.5}` | Center alignment |
| `new_batch` | `true` | Required for text visibility on `show_bg:true` containers |
| `empty_text` | `"Type here..."` | Placeholder for TextInput |

### 4.10 Available But Not Interactive via Synthetic Clicks

| Widget | Limitation |
|--------|-----------|
| **`Slider`** | `on_change` needs mouse drag — can't trigger via synthetic MouseDown/MouseUp |
| **`DropDown`** | Popup menu is separate overlay window — can't select items synthetically |

### 4.11 Not in Build

| Widget | Behavior |
|--------|----------|
| **`TabBar`** / **`Tab`** | width=0, height=0 — no visible output |

### 4.12 Validation

The pi extension pre-validates splash bodies before launch. Catches: unknown widgets, multiline string literals, undeclared named references, parenthesized `if (cond)`, TextInput without fixed height, `on_render:`, top-level function calls.

---

## 5. Verified Patterns (Tested 2026-06-24)

All patterns verified end-to-end via extension tools.

| Pattern | Test Status | Test Data |
|---------|-------------|-----------|
| Direct orphan button → `__pi_response.set_text()` | ✅ | Button at (26,135,928,200), click at (490,235) → doc: `"clicked:1"` |
| Nested button inside container without padding | ✅ | Buttons at (447,135,17,22), click at (457,146) → counter decremented to -1 |
| Counter variable persistence | ✅ | Click - → 0→-1, click + → -1→0, Send → doc: `"count:0"` |
| Toggle (same-value via version counter) | ✅ | "true" → "true" → "false" → "false" all delivered |
| `type_text` → click Submit | ✅ | "hello world" typed, submitted → doc: `"got:hello world"` |
| `send_pi_response` → splash reads data | ✅ | "Data from pi agent!" appears in __pi_data and __ai_text |
| Dynamic list via `set_text()` | ✅ | 2 items added → doc: `"1. Buy groceries\\n2. Write tests"` |
| Coordinate shift after layout change | ✅ | Buttons shifted +19px after 2nd list item added |
| Container padding clipping | ❌ | RoundedView{padding:16} → buttons overflow padded area → unhittable |

---

## 6. Known Current Limitations

| Limitation | Workaround |
|-----------|------------|
| `debug_response` may arrive repeatedly | Accept first response, ignore duplicates |
| `pending_click` is a single slot — two rapid clicks overwrite | Take a fresh `widget_snapshot` between clicks |
| `wait_for_response` may time out | Use `inspect_makepad_doc` (synchronous) instead |
| Widget text shows `" "` (space) instead of `""` for `__pi_response` | Use `value` field for TextInput, not `text` field |
| Stale content after rapid close+launch | Wait 1-2 seconds between close and launch |
| Debug commands freeze after ~50 ops | Kill both processes, rebuild, restart |
| Coordinates shift after layout changes | Always take a fresh `widget_snapshot` before each click |
| `type_text` fills first TextInput in splash body | Check `value` field on orphan TextInputs in snapshot |
| Container padding clips children's hit areas | Keep buttons as direct orphans (no container wrapping) |
| Orphan coordinates are window-relative | Use directly from dump/snapshot — no window offset needed |
| `RadioButton`, `ToggleFlat`, `CheckBox` variables don't persist | Use `ButtonFlat` with manual toggle |
| Background sub-agent slow (5-20s API call) | Wait for response; check harness logs |
| `__ai_text` is a TextInput — fills before user's in `type_text` | Put user's TextInput FIRST in splash body (default is correct) |
| Sub-agent session dispose warning | Call `stop_background_session` when done |

### Recovery from Debug Freeze

If debug commands return `"No result provided"` or time out after heavy use:
1. `pkill -f makepad-host; pkill -f harness`
2. `cargo build -p harness -p makepad-host`
3. Launch a new app

---

## 7. Build, Test, Logs

### Build

```bash
cargo build -p harness
cargo build -p makepad-host
```

### Test

```bash
# Rust integration test (headless harness)
cargo test -p harness --test integration_smoke

# TypeScript integration test (requires running harness + makepad-host)
cd .pi/extensions/makepad && npm test
```

### Logs

Both processes output to stderr via `eprintln!`. Prefixes: `[harness]`, `[makepad-host]`, `[splash]`. makepad-host is spawned with `Stdio::inherit()`, so its logs go to the pi terminal.

---

## 8. Test Walkthrough Protocol

When walking through apps step by step:
1. For each step, explain what you're about to do and what the user should see
2. **Wait for confirmation** before executing
3. Keep steps small — one interaction per confirmation
4. Always show coordinates before clicking
5. Only move to next step when user confirms current step is complete

## 9. End of Task

At the end of a task, suggest a commit message to the user based on the current diff.

## 10. Test Results Archive (2026-06-24)

All core patterns were tested end-to-end. The following findings correct earlier documentation:

### Coordinate System Correction

**OLD claim:** Orphan widget coordinates are parent-relative.
**REALITY:** Orphan widget coordinates in `widget_dump` and `widget_snapshot` are **window-relative** (relative to window content origin). Use them directly for click coordinates.

**Proof:** AgentSplash at window-relative (26, 132). Orphan outer View at dump (26, 132) — exact match. Orphan "-" button at dump (447, 135) — click at center (457, 146) hit the button ✅

### Container Clipping Correction

**OLD claim:** Nested buttons work identically to direct orphans.
**REALITY:** Containers with `padding:16` and `show_bg:true` create draw clips. Buttons overflowing the padded area have reduced `clipped_rect` → hit-test fails.

### wait_for_response Timing

**OLD claim:** Primary way to receive responses.
**REALITY:** May time out during tool transitions. Use `inspect_makepad_doc` for reliable synchronous checking.

### type_text Walk Order

**OLD claim:** Walks orphan splash widgets.
**REALITY:** Walks AgentSplash's `try_children()` → inner View's children list. Fills first TextInput depth-first.

### Verified Patterns Summary

| Pattern | Status |
|---------|--------|
| Direct orphan button → `__pi_response.set_text()` | ✅ Click at (490, 235) → doc: `"clicked:1"` |
| Nested button (no-padding container) → counter | ✅ Click at (457, 146) → count: 0 → -1 |
| Toggle (same-value via version counter) | ✅ All four same/different values delivered |
| type_text → button → response | ✅ "hello world" → doc: `"got:hello world"` |
| send_pi_response → splash reads | ✅ Data appears in __pi_data and __ai_text |
| Dynamic list set_text() | ✅ 2 items added, Done button returned both |
| Coordinate shift after layout | ✅ Buttons shifted +19px after 2nd list item |
| Container padding clipping | ❌ RoundedView{padding:16} → unhittable buttons |
