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
- `background-agent.ts` — sub-agent sessions, auto-handler, streaming delta dispatch
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
{"type": "send_streaming_delta", "app_id": "todo-1", "delta": "hel"}
{"type": "send_streaming_end", "app_id": "todo-1", "final_text": "hello world"}
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

#### Streaming Response (ai:ask → live deltas → splash)
1. Splash calls `ui.__pi_response.set_text("ai:ask:message")` → AgentSplash writes `user_response` + increment `user_response_version`
2. Harness bridge loop detects version change → forwards `{"type":"user_response",...}` to pi over JSON WS
3. Extension auto-handler matches `ai:ask:` prefix → finds session via `appSessionMap.get(appId)`
4. Auto-handler creates a **per-prompt subscription** (same proven pattern as `send_background_message`) that:
   - Captures each `text_delta` event from the pi SDK
   - Sends it immediately as `{"type":"send_streaming_delta","app_id":"...","delta":"<raw new chars>"}` over JSON WS
5. Harness receives `send_streaming_delta` → **APPENDS** the delta to `streaming_text` CRDT field:
   ```rust
   let existing = agent.streaming_text.unwrap_or_default();
   agent.streaming_text = Some(existing + &delta);
   ```
6. CRDT syncs to makepad-host → `samod` fires a **Signal** event
7. AgentSplash.handle_event() calls `sync_streaming_text()` (only on `Event::Signal`, not on 60fps Draw events):
   - Reads `streaming_text` from doc → compares to `self.last_streaming_text`
   - If changed: updates `__ai_text` TextInput and `log` Label with the full accumulated text
   - The `log` widget update uses `rfind("\n🤖 ")` to correctly find the AI response boundary even when the response contains internal newlines (bullet lists, paragraphs)
8. On sub-agent completion, auto-handler calls `unsub()` → sends `{"type":"send_streaming_end","app_id":"...","final_text":"..."}`
9. Harness handles `send_streaming_end`: sets `pi_response = Some(final_text)`, clears `streaming_text = None`, sets `extension_requests = true`
10. CRDT sync → Signal → `sync_pi_data_to_splash()` reads `pi_response`, writes final text to `__ai_text`, `__pi_data`, and `log` (replacing the "🤖 ..." line), then **clears** `pi_response` from the doc

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
    /// Accumulated streaming text from sub-agent deltas.
    /// Harness APPENDS each delta on send_streaming_delta (raw new chars),
    /// so this field grows over time. Read by makepad-host's
    /// sync_streaming_text() for live display. Cleared by send_streaming_end
    /// (which also sets pi_response with the final text).
    pub streaming_text: Option<String>,
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

### AgentSplash `handle_event` (Signal-Only Sync)

AgentSplash follows a different event pattern from the host. Its `handle_event` is:

```rust
fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
    self.view.handle_event(cx, event, scope);
    self.redraw(cx);

    // Check __pi_response label for splash → pi communication
    let response_widget = self.widget(cx, &[id!(__pi_response)]);
    if !response_widget.is_empty() {
        let current = response_widget.text();
        if current != self.last_response && !current.is_empty() {
            self.last_response = current;
            write_doc_field("user_response", current.clone());
        }
    }

    // Drain mpsc streaming channel (direct path, not CRDT)
    if let Some(rx) = STREAMING_RX.get() {
        while let Ok(delta) = rx.lock().try_recv() {
            // update __ai_text and log widget
        }
    }

    // CRDT sync — ONLY on Event::Signal to avoid 60fps doc reads
    if matches!(event, Event::Signal) {
        self.sync_streaming_text(cx);
        self.sync_pi_data_to_splash(cx);
    }
}
```

Key differences from the host's handle_event:
- **`sync_streaming_text` and `sync_pi_data_to_splash` only run on `Event::Signal`** — not on Draw, Mouse, or Timer events. This prevents reading+hyrating the CRDT doc at 60fps (which caused CPU spikes during streaming).
- The `__pi_response` check runs on ALL events (not just Signal) so that splash → pi messages are not missed.
- The STREAMING_RX mpsc channel (a secondary direct-delivery path) is drained on every event.

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

#### `ai:init:<system_prompt>` (App-Provided System Prompt)

The splash app can set its own system prompt for the sub-agent session by sending:
```splash
ui.__pi_response.set_text("ai:init:You are an Italian chef expert...")
```

When the auto-handler receives `ai:init:`, it:
1. Disposes any existing session for this app
2. Creates a new DeepSeek V4 Flash session
3. Seeds the conversation with the app's system prompt as context
4. Associates the new session with the app_id for subsequent `ai:ask:` messages
5. Sends back `[Session initialized with app-provided system prompt]` confirmation

This allows multiple apps to coexist, each with its own AI personality:
```splash
prompt_inp := TextInput{width:Fill height:34 empty_text:"Your AI personality..."}
init_btn := ButtonFlat{text:"Set Prompt & Start" on_click:||{
    let p = ui.prompt_inp.text()
    if p == "" { p = "You are a helpful assistant." }
    ui.__pi_response.set_text("ai:init:" + p)
}}

inp := TextInput{width:Fill height:34 empty_text:"Ask something..."}
send_btn := ButtonFlat{text:"Send" on_click:||{
    let m = ui.inp.text()
    if m != "" { ui.__pi_response.set_text("ai:ask:" + m); ui.inp.set_text("") }
}}
```

**Note:** The system prompt is seeded via conversation context (first message) because `createAgentSession` in the pi SDK does not expose a `systemPrompt` parameter directly. The first prompt sent is `[SYSTEM CONTEXT] <system_prompt>`.

#### Blank-Slate Sessions (No Inherited Context)

Sub-agent sessions created via `ai:init:`, `launch_app_with_agent`, `start_background_session`, or the `ai:ask:` auto-fallback DO NOT inherit the main agent's system prompt, AGENTS.md, skills, or any other context. This is enforced by `getBlankSlateResourceLoader()` in `background-agent.ts`:

| Override | Effect |
|----------|--------|
| `noContextFiles: true` | No AGENTS.md/CLAUDE.md from cwd or agent dir |
| `noSkills: true` | No skill prompts injected |
| `noPromptTemplates: true` | No file-based prompt templates |
| `noThemes: true` | No theme-driven prompts |
| `noExtensions: true` | No extension hooks |
| `systemPromptOverride: () => ""` | System prompt forced to empty string |
| `agentsFilesOverride: () => ({ agentsFiles: [] })` | Explicitly empty context files |
| `cwd / agentDir: <tmpdir>` | Isolated temp directory — no project config leaks |

Result: the sub-agent has **no knowledge** it is a coding agent. It is a blank AI assistant. The splash app controls its personality entirely via `ai:init:<prompt>`. If no init is sent, the auto-fallback uses a minimal default ("You are a helpful background AI assistant. Be concise and accurate.").

### Auto-Handler (Extension Side)

The extension registers an `onMessage` handler at startup (via `startAutoBackgroundHandler()` in `index.js`) that intercepts all `user_response` messages from the harness. When the response starts with `ai:`, it dispatches to `handleAutoMessage()` which supports:

| Protocol | Purpose |
|----------|---------|
| `ai:init:<prompt>` | Create/replace session with app-provided system prompt |
| `ai:ask:<message>` | Send message to existing session, forward response |
| `ai:start` | (legacy) Auto-create session |

If no session exists when `ai:ask:` arrives, one is auto-created with a default prompt.

### Auto-Display via `__ai_text` (with Streaming)

The AgentSplash injects a `__ai_text := TextInput{text:" " height:0 width:Fill visible:false}` widget that
auto-displays the sub-agent's response — no manual reading needed.

**Streaming architecture (per-prompt subscription):** When the auto-handler processes
an `ai:ask:` message, it creates a **per-prompt subscription** (identical pattern to
`send_background_message`):

```typescript
let response = "";
const unsub = stored.session.subscribe((event: any) => {
  if (event.type === "message_update" &&
      event.assistantMessageEvent?.type === "text_delta") {
    const delta = event.assistantMessageEvent.delta;
    response += delta;
    sendToHarness({ type: "send_streaming_delta", app_id: appId, delta: delta });
  }
});
await stored.session.prompt(message, { expandPromptTemplates: false });
unsub();
sendToHarness({ type: "send_streaming_end", app_id: appId, final_text: response });
```

Key characteristics:
- **Individual deltas** are sent (raw new chars, NOT full accumulated text)
- The harness **APPENDS** each delta to the CRDT `streaming_text` field
- On completion, `send_streaming_end` sets `pi_response` (final text) and clears `streaming_text`
- `sync_streaming_text()` is called **only on `Event::Signal`** (not on every Draw/Mouse event)
  to avoid reading+hyrating the CRDT doc at 60fps
- The `log` widget uses `rfind("\n🤖 ")` (not `rfind('\n')`) to correctly find the
  AI response boundary when the response contains internal newlines

**Also has a session-level subscription** (`setupSessionStreaming`) that silently
accumulates deltas into `stored.accumulated` (does NOT send to harness). This is
a fallback for `send_background_message` tool usage and future use.

**Result:** the `__ai_text` widget shows text appearing token-by-token as the model
generates, rather than waiting for the full response. The splash app can also read
the final response via `ui.__pi_data.text()` (only updated on completion).

### Injected Widgets

| Widget ID | Type | Purpose |
|-----------|------|---------|
| `__pi_response` | `Label{text:""}` (hidden) | Splash writes to send responses to pi |
| `__pi_data` | `Label{text:" "}` (hidden) | Splash reads to get data from pi |
| `__ai_text` | `TextInput{height:34 width:Fill}` (visible) | Auto-displays AI response from sub-agent |

### Workflow

#### Option A: Pre-created session
1. **Create sub-agent**: `start_background_session(provider="deepseek", model_id="deepseek-v4-flash", system_prompt="...")`
2. **Launch app with session**: `launch_makepad_app(app_id="my-app", splash_body="...", agent_session_id="<sid>")`
3. **User sends message**: splash calls `ui.__pi_response.set_text("ai:ask:" + msg)`
4. **Auto-handler** (extension) detects `user_response` → routes to sub-agent → calls `session.prompt()`
5. **Response sent back**: auto-handler calls `sendToHarness({ type: "send_pi_response", ... })`
6. **Harness writes doc**: `pi_response = "..."` + `extension_requests = true`
7. **Signal fires** → `sync_pi_data_to_splash` reads doc → `__ai_text.set_text(response)`
8. **Response visible** on screen automatically

#### Option B: App-provided system prompt (`ai:init:`)
1. **Launch app**: `launch_makepad_app(app_id="my-app", splash_body="...")` (no session needed)
2. **App sends init**: splash calls `ui.__pi_response.set_text("ai:init:" + systemPrompt)`
3. **Auto-handler** creates a new DeepSeek session, seeds it with the system prompt, associates it with this app
4. **App sends message**: splash calls `ui.__pi_response.set_text("ai:ask:" + msg)`
5. Response flows as in Option A steps 4-8

#### Option C: Convenience tool
1. Use `launch_app_with_agent(app_id="my-app", splash_body="...", system_prompt="...")` — creates session + launches app in one step

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

### 3.4 Inline Runsplash Rendering (Current Working Implementation)

As of the latest session, runsplash code can be rendered **inline** inside the chat app
via a nested AgentSplash widget injected into every splash body's `SPLASH_SUFFIX`.

**How it works:**
1. `SPLASH_SUFFIX` includes `__run_splash := AgentSplash{width:Fill height:Fit is_root:false}`
2. The nested AgentSplash has `is_root:false`, so it does NOT sync from the CRDT doc
3. During streaming, `sync_streaming_text()` extracts `\`\`\`runsplash` blocks from
   the accumulated text and calls `run_splash.set_text(cx, &runsplash_code)`
4. The nested AgentSplash evaluates the runsplash code and renders it **inline**
   below the chat app (preserving the chat state)
5. Log shows "⚙ Generating..." during streaming, "✅ Done" on completion
6. `set_text()` saves the previous body and restores it on eval failure (silently
   keeps the last valid UI when partial code doesn't parse)

**Known Problems:**
1. **NaN crash on content growth** — When the nested AgentSplash's content grows from
   partial (label + "0") to full (with buttons), the parent View's stale layout produces
   NaN. The crash is in `move_align_list` (turtle.rs:2342). The `catch_unwind` approach
   doesn't help because the NaN persists in the layout state.
2. **Second prompt error** — Sending a second `ai:ask:` while the first is still
   streaming produces "Agent is already processing". Fixed by adding
   `streamingBehavior: "steer"` to `session.prompt()` in background-agent.js.
3. **Nested children invisible in debug tools** — `widget_snapshot` and `widget_dump`
   only show the nested AgentSplash widget itself, not its rendered children (buttons,
   labels). The children are in the VM's widget tree, separate from the main tree.
4. **Counter buttons overflow Fit height by ~4px** — The nested counter UI needs
   ~258px but gets only 251px, clipping the bottom of the buttons.

**Suggested fix (unproven):** Don't render partial code during streaming at all — only
render on completion when the closing \`\`\` arrives. Use "⚙ Generating..." as a
placeholder during streaming. This avoids the layout growth issue entirely.

---

## 4. Splash DSL Guide (General Reference)

This section covers general Makepad Splash DSL patterns that apply to ANY app body.

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
| **`ButtonFlat`** | Click → variable write, `set_text()`, `text()` | All interactive controls |
| **`Button`** | Click → variable write, `set_text()`, `text()` | Standard buttons |
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
ButtonFlat{text:"Show" on_click:||{ui.display.set_text("Current: " + toggled)}}
```



### 4.7 Patterns

#### 4.7.1 Struct Arrays & Array Operations

The Splash VM supports arrays of structs with `.push()`, `.remove()`, `.len()`, and `.retain()`. Read fields via `array[index].field`, update with `array[index] += {field: val}`.

**⚠️ `for i in items` iterates over VALUES, not indices.** This is a critical gotcha — `for i in items` behaves like a for-each loop, so `i` is the element value (string), not an integer index. Using `items[i]` will silently mis-index (treating a string as an index, which falls through to the first element):

```splash
// ❌ WRONG — i is the string value, not an index
for i in items { out = out + items[i] }  // always returns items[0]

// ✅ CORRECT — use while loop with explicit index
let idx = 0
while idx < items.len() {
    out = out + items[idx]
    idx = idx + 1
}

// ✅ ALSO CORRECT — direct indexing when you know the position
items[0]  // works
items[1]  // works
```

**Note:** `while` loops in the Splash VM are functional but can cause debug system timeouts with rapid successive clicks. After using `while` in an `on_click`, allow 10+ seconds for the debug system to recover.

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
```

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

### 4.10 Not in Build

| Widget | Behavior |
|--------|----------|
| **`TabBar`** / **`Tab`** | width=0, height=0 — no visible output |

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
| Array push + indexing (while loop) | ✅ | 3 pushes → items[0..2] → doc: `"Alpha, Beta, Gamma"` |
| Coordinate shift after layout change | ✅ | Buttons shifted +19px after 2nd list item added |
| Container padding clipping | ❌ | RoundedView{padding:16} → buttons overflow padded area → unhittable |
| Sub-agent `ai:ask:` auto-handler (pre-created session) | ✅ | Type text → click Send → `__ai_text` shows AI response (2026-06-24) |
| Sub-agent via `launch_app_with_agent` (system_prompt) | ✅ | "What is 2+2?" → AI: "**2 + 2 = 4**" in `__ai_text` (2026-06-29) |
| `send_pi_response` → splash reads `__pi_data` | ✅ | "Greetings from pi!" → label shows "Got: Greetings from pi!" |
| Splash → Pi communication (`__pi_response.set_text`) | ✅ | Click "Send to Pi" → doc: `"hello from splash"` |
| Two-way comms (pi→splash + splash→pi) | ✅ | Full round-trip verified in single session |
| Per-prompt streaming (individual deltas) | ✅ | `send_streaming_delta` sends raw new chars; harness APPENDS to CRDT (2026-07-06) |
| Log widget no duplicate lines | ✅ | `rfind("\n🤖 ")` correctly handles AI text with internal newlines (2026-07-06) |
| Signal-only CRDT sync (no CPU jank) | ✅ | Doc read+hyrdate only on `Event::Signal`, not 60fps Draw events (2026-07-06) |
| Counter with no bg agent | ✅ | All buttons (+/-/Reset/Send) work; `__pi_response.set_text("count:2")` delivers (2026-07-06) |
| Todo list via `set_text()` (no bg agent) | ✅ | Add items, scroll, "Send to Pi" sends list content (2026-07-06) |

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
| `ai:init:` needs extension restart to pick up new code | Restart pi after recompiling `background-agent.ts` → `dist/background-agent.js` |
| Auto-handler runs with cached extension code | Extension compiled dist is loaded at pi startup; recompiling dist only takes effect on next pi session |
| `createAgentSession` has no `systemPrompt` parameter | **FIXED**: Sub-agent sessions now use a blank-slate `ResourceLoader` with `noContextFiles`, `noSkills`, `noExtensions`, and `systemPromptOverride: () => ""`. The splash app's system prompt (via `ai:init:<prompt>`) is seeded as `[SYSTEM CONTEXT] <prompt>` on an otherwise empty session. See Section 3.1. |
| `for i in items` iterates over values (not indices) in Splash VM | Use `while idx < items.len()` with `items[idx]` for correct indexing |
| `while` loops in Splash can cause debug system timeouts | Allow 10s+ cooldown after using `while` in `on_click`; avoid rapid successive clicks after while loops |
| Standalone `ScrollBars`/`ScrollBar` as child widget (historically) | **No longer reproducible** (tested 2026-07-01 on clean build). The Splash VM now handles ScrollBars gracefully — renders as zero-size when used standalone. The fix was likely in the Makepad upstream update between git revisions. The `catch_unwind` wrapper in `app.rs` (commit b965536) provides defense-in-depth against any future panics. |
| `View{scroll_bars: ScrollBars{...}}` — scroll_bars as View PROPERTY works | ✅ The View manages scroll internally. Use: `View{width:Fill height:300 scroll_bars: ScrollBars{show_scroll_x:false show_scroll_y:true scroll_bar_y: ScrollBar{drag_scrolling:true}} ...}` |
| **Streaming now working (2026-07-06)** | **FIXED**: Per-prompt subscription streams individual deltas via `send_streaming_delta`, harness APPENDS to CRDT `streaming_text`, makepad-host syncs on `Event::Signal` only. Log widget correctly replaces the "🤖 ..." line using `rfind("\n🤖 ")` instead of `rfind('\n')` to handle AI text with internal newlines. See Section 3 for architecture. |
| **CPU jank during streaming** | **FIXED (2026-07-06)**: `sync_streaming_text()` and `sync_pi_data_to_splash()` now only run on `Event::Signal`, not on every 60fps Draw/Mouse event. This eliminated the CRDT doc read+hydrate loop that was causing janky UI. |
| `createAgentSession` inherits parent system prompt (historical) | **FIXED (2026-07-01)**: `getBlankSlateResourceLoader()` creates an isolated `DefaultResourceLoader` pointing at a temp directory with all context/skills/prompts/extensions disabled. The sub-agent no longer inherits the main agent's AGENTS.md, SYSTEM.md, skills, or any other context. See Section 3.1 for implementation details. |
| Programmatic auto-scroll via `ScrollEvent` has no effect | `scroll_bars` only respond to touch/mouse gesture events, not programmatic `ScrollEvent` dispatch. Manual scrolling still works. |
| **Makepad-host crash: `dy.is_nan()` in `turtle.rs:2342` during streaming** | **FIXED (2026-07-02)**: The `SPLASH_PREFIX` in `agent_splash.rs` was missing `width:Fill` on the outer wrapper View. This caused `View{height:Fit flow:Down <body> __ai_text{width:Fill height:0}}` — a parent with no explicit width containing children with `width: Fill`. During sub-agent streaming responses, text written to `__ai_text` triggered a re-layout that produced NaN in `turtle.total_resolved_length_to()` → `move_align_list(dy=NaN)`. Fix: added `width: Fill` to `SPLASH_PREFIX`. See Section 11 for full analysis. |
| **AI text with internal newlines duplicates log lines** | **FIXED (2026-07-06)**: `sync_streaming_text()` and `sync_pi_data_to_splash()` now use `rfind("\n🤖 ")` instead of `rfind('\n')` to find the AI response boundary. The old code found newlines INSIDE the AI response (e.g., after bullet points), creating duplicate "🤖 ..." lines. |

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
| Sub-agent auto-handler (`ai:ask:` with pre-created session) | ✅ Type text → click Send → `__ai_text` displays AI response |
| `send_pi_response` → `__ai_text` auto-display | ✅ "Test message from pi to splash app" appeared in `__ai_text` and `__pi_data` within seconds |

## 11. Crash Analysis: `dy.is_nan()` in `move_align_list` (2026-07-02)

### Symptom

```
thread 'main' panicked at makepad/draw/src/turtle.rs:2342:9:
assertion failed: !dy.is_nan().0M
```

Stack trace: `received_timer` → `call_draw_event` → `handle_event` → `draw_walk` → `View::draw_walk` → `AgentSplash::draw_walk` → `View::draw_walk` → `draw_bg.end()` → `end_turtle` → `end_turtle_with_guard` → `move_align_list(..., dy=NaN, ...)`

### Trigger

Launching a splash app via `launch_app_with_agent`, then clicking a button that sends an `ai:ask:` message. When the sub-agent streams its response back and text is written to the injected `__ai_text` widget, the Makepad layout engine recalculates positions and hits the NaN assertion.

### Root Cause

The `SPLASH_PREFIX` in `makepad-host/src/agent_splash.rs` wrapped every splash body in an outer View **without `width: Fill`**:

```rust
// BEFORE (broken):
const SPLASH_PREFIX: &str = "use mod.prelude.widgets.*View{height:Fit flow:Down ";
```

This produced the following widget hierarchy:
```
View{height:Fit flow:Down          ← NO width! width computed from children
  <user's RoundedView{width:Fill ...}>
  __ai_text := TextInput{width:Fill height:0 visible:false}
  __pi_response := Label{...}
  __pi_data := Label{...}
}
```

When `sync_pi_data_to_splash()` or `sync_streaming_text()` called `set_text()` on `__ai_text`, Makepad's layout engine attempted to resolve `width: Fill` on a child whose parent had no explicit width. This circular dependency (`Fill` depends on parent width, parent width depends on child) produced `NaN` in `turtle.total_resolved_length_to()`, which flowed through `end_turtle_with_guard` → `move_align_list(dy=NaN)` → assertion failure.

**Why it was intermittent:** The crash only occurred when text was actively written to `__ai_text` during streaming deltas while a Draw event was in flight. The timing of the streaming relative to the draw cycle determined whether the NaN would manifest.

**Secondary finding:** `visible:false` on `__ai_text` (a TextInput) is not respected — every snapshot shows it as `visible: true` with full width. This means the injected TextInput always participates in layout.

### Fix

Added `width: Fill` to the outer wrapper View so children with `width: Fill` resolve correctly:

```rust
// AFTER (fixed):
const SPLASH_PREFIX: &str = "use mod.prelude.widgets.*View{width:Fill height:Fit flow:Down ";
```

This gives the outer View a resolved width from the parent (the `splash_holder` in the window), so all `width: Fill` children compute their widths from a known value — no circular dependency.

### Verification

1. Launched `file-summarizer` app with `launch_app_with_agent` and a large system prompt
2. Typed a filename into the TextInput
3. Clicked "Summarize" button → `__pi_response` set to `ai:ask:summarize shared/src/lib.rs`
4. Sub-agent began streaming response → text written to `__ai_text` via CRDT
5. **No crash** — app survived the full streaming flow

### Code Change

**File:** `makepad-host/src/agent_splash.rs`, line 44

```diff
-const SPLASH_PREFIX: &str = "use mod.prelude.widgets.*View{height:Fit flow:Down ";
+const SPLASH_PREFIX: &str = "use mod.prelude.widgets.*View{width:Fill height:Fit flow:Down ";
```

---

## 12. Lessons Learned (2026-07-06 Session)

This section documents approaches that were tried but did not work, to avoid repeating the same dead ends.

### 12.1 Nested AgentSplash causes NaN layout crashes

**Attempted:** Adding `__run_splash := mod.widgets.AgentSplash{width:Fill height:Fit is_root:false}` to SPLASH_SUFFIX so that runsplash code could be rendered inline via a nested AgentSplash, preserving the chat app body.

**Result: FAILED** — consistently produces `assertion failed: !dy.is_nan()` in `turtle.rs:2342` (`move_align_list`). The crash happens during `draw_walk` (rendering), not during `set_text` (evaluation). The nested AgentSplash's `width:Fill` creates a circular layout dependency with the parent View's `height:Fit`, and when the nested content grows after `set_text`, the parent's stale layout produces NaN.

**`catch_unwind` around `draw_walk` does NOT fix it:** The NaN value persists in the turtle/layout state even after the panic is caught. The next draw cycle in the parent View (KeyboardView) encounters the same NaN and crashes again.

### 12.2 Partial runsplash code evaluation during streaming is unreliable

**Attempted:** Extracting runsplash code from the accumulated streaming text BEFORE the closing ``` arrives, and trying to evaluate the partial code progressively.

**Result: FAILED** — Partial Splash code almost never parses because the DSL requires complete syntax (balanced braces, complete property names, etc.). The only time it works is when the code inside the block happens to be syntactically complete before the closing ``` (e.g., when the AI finishes the closing `}` before writing the closing ```). Most of the time, eval fails and the body is restored to the previous state.

**Practical limit:** The "Generating..." status message in the log is about as much feedback as you can show during streaming. The rendered UI only reliably appears when the complete `\`\`\`` closing marker arrives.

### 12.3 Inline rendering via body replacement loses chat state

**Attempted:** When runsplash code is detected in `sync_pi_data_to_splash`, calling `self.set_text(cx, &runsplash_code)` to replace the entire splash body.

**Result: Works but destructive** — The chat app is replaced entirely. All `let` variables (messages array, counters) are lost because the Splash VM re-evaluates from scratch. The user can see the rendered UI but the chat context is gone.

### 12.4 Version counters add complexity without clear win

**Attempted:** Adding `pi_response_version` and `streaming_text_version` fields to `AgentDoc` to avoid full-doc hydration on every frame. The sync functions first read just the version counter (cheap), and only do full `hydrate` if the version changed.

**Result: NOT RECOMMENDED** — The performance improvement was marginal (the `hydrate` call was already fast enough). The version counters added complexity to the `shared::AgentDoc` struct, required changes in the harness, and introduced new failure modes (version mismatches, forgotten increments). The original approach of reading and hydrating the full doc on each Signal is simpler and more reliable.

### 12.5 AI system prompt must be short

**Attempted:** Including the entire Splash DSL reference guide (all widgets, properties, examples) in the system prompt.

**Result: Counterproductive** — Long prompts overwhelm the model and produce worse results (missing buttons, wrong syntax). A short prompt with exactly one working counter example consistently produces better code.

### 12.6 `streamingBehavior: 'steer'` fixes second-prompt error

**Attempted:** Sending a second `ai:ask:` message while the first was still streaming produced `"Agent is already processing"` error.

**Result: WORKED** — Adding `streamingBehavior: "steer"` to the `session.prompt()` call in `background-agent.js` correctly cancels the in-progress generation and starts fresh with the new message. No crash, no error.

### 12.7 `wait_for_response` hang fix

**Attempted:** `wait_for_response` never fired for `ai:ask:` responses because the auto-handler only set `pi_response`, not `user_response`.

**Result: WORKED** — Adding `agent.user_response = Some(final_text)` + `agent.user_response_version += 1` to the `SendStreamingEnd` handler in the harness causes the bridge loop to forward the response to the extension, which triggers `wait_for_response`.

### 12.8 Error fallback should not be shown inline

**Attempted:** When `eval_body` fails (e.g., partial code during streaming), rendering `SPLASH_ERROR_FALLBACK` (dark red box with "Splash app could not be rendered") inside the nested AgentSplash.

**Result: BAD UX** — The error fallback covers up any partially-rendered content and looks broken. Better to silently restore the previous valid body (as `set_text` now does by saving `prev_body` and re-evaluating it on failure).

### 12.9 `draw_walk` must not panic

**Attempted:** Letting the nested AgentSplash's `draw_walk` panic propagate up to `catch_unwind` in the host's `handle_event`.

**Result: NOT ENOUGH** — The approach of wrapping individual widget `draw_walk` calls with `catch_unwind` does NOT prevent subsequent crashes because the NaN state persists in Makepad's turtle/layout system. The parent View encounters the same NaN on the next draw cycle. The only reliable fix is to prevent NaN from entering the layout in the first place (avoid `width:Fill` / `height:Fill` combinations that create circular dependencies).

### 12.10 NaN crash persists even with SPLASH_PREFIX `width:Fill` fix

**Attempted:** The `dy.is_nan()` crash in `move_align_list` (turtle.rs:2342) keeps happening on every draw event even after adding `width:Fill` to SPLASH_PREFIX. The crash is 100% reproducible with any app that uses the SPLASH_SUFFIX widgets (`__ai_text`, `__pi_response`, `__pi_data`).

**Root cause not found** — The crash might be from the `__ai_text := TextInput{text:" " height:0 width:Fill visible:false}` widget. A TextInput with `height:0` and `width:Fill` inside a `height:Fit flow:Down` parent View might create a layout conflict. The text " " (space) has font height > 0, conflicting with `height:0`.

**Clean rebuild sometimes fixes it** — Running `cargo clean && cargo build` resolved the crash for one session, suggesting stale incremental build artifacts can cause the NaN.

### 12.11 `set_text` body restoration prevents error display

**Attempted:** When `eval_body` fails (partial/incomplete Splash code), the original code rendered `SPLASH_ERROR_FALLBACK` (dark red box). Changed `set_text` to save `prev_body` before eval and restore on failure.

**Result: WORKS** — The previous valid body is re-evaluated and displayed on eval failure. No more red error boxes. The user only sees the last working UI, with partial/failed states silently skipped.

### 12.12 `eval_body` must not render error fallback

**Attempted:** `eval_body` called `render_body(cx, SPLASH_ERROR_FALLBACK)` on failure, which rendered a dark red error box over the entire splash area.

**Result: Changed to just return false** — The caller (`set_text`) handles restoration. The error fallback constant is now dead code.

### 12.13 Partial inline render succeeds, full render crashes with NaN

**Observed behavior:** During streaming, the inline `__run_splash` AgentSplash successfully renders partial code (e.g., just the counter label and "0" display). But when the next streaming delta adds more content (buttons, layout), the re-evaluation triggers a NaN crash in `move_align_list`.

**Root cause:** The nested AgentSplash starts with empty content (0 height). Partial code evaluates successfully and renders at the computed height. The parent View's layout is computed with this height. When `set_text` is called again with more complete code (taller content), the nested AgentSplash grows, but the parent View's layout is stale. This creates a circular dependency: the nested widget needs more space than allocated, producing NaN.

**Failed workarounds:**
- `catch_unwind` around `set_text` doesn't help because the NaN happens in the subsequent `draw_walk`, not in `set_text`
- `self.redraw(cx)` after `set_text` doesn't trigger a full re-layout — the parent View reuses its cached child positions
- Giving the nested AgentSplash `height:Fit` doesn't fix it because Fit is computed from content, but the parent already decided the height

**Hypothesis for fix (unproven):** Don't render partial code during streaming at all. Only render on completion (when the closing ``` arrives). Use a simple "⚙ Generating..." status during streaming. This avoids the layout growth issue entirely.
