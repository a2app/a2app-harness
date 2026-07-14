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

## 2. Communication Flows

### 2.1 JSON WS Protocol (pi ↔ harness, port 2341)

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

### 2.2 Launch App

1. pi sends `{"type":"launch","app_id":"...","splash_body":"..."}` over JSON WS
2. Harness writes `pending_app` to CRDT doc (Pending → Launched)
3. CRDT syncs to makepad-host over samod WS
4. Makepad-host renders splash in AgentSplash widget on next Draw event

### 2.3 User Response (splash → pi)

1. Splash app calls `ui.__pi_response.set_text("data")` in any `on_click` handler
2. AgentSplash detects the label text changed → writes `user_response` to CRDT doc
3. AgentSplash also increments `user_response_version` before writing
4. Harness bridge loop compares version number (not value) to detect changes
5. Harness forwards `{"type":"user_response","app_id":"...","response":"..."}` to pi
6. Pi extension buffers the event (per-type Map) and dispatches to `wait_for_response`

### 2.4 Pi Response (pi → splash)

1. pi (or extension auto-handler) sends `{"type":"send_pi_response","app_id":"...","data":"..."}` over JSON WS
2. Harness writes `pi_response` to CRDT doc + sets `extension_requests = true`
3. CRDT syncs to makepad-host over samod WS
4. Background thread detects `pi_response` change → signals UI thread
5. AgentSplash reads `pi_response`, writes it to `__ai_text` widget (TextInput) and `__pi_data` label
6. Splash app reads response via `ui.__ai_text.text()` or `ui.__pi_data.text()`

### 2.5 Streaming Response (ai:ask → live deltas → splash)

1. Splash calls `ui.__pi_response.set_text("ai:ask:message")` → AgentSplash writes `user_response` + increment `user_response_version`
2. Harness bridge loop detects version change → forwards `{"type":"user_response",...}` to pi over JSON WS
3. Extension auto-handler matches `ai:ask:` prefix → finds session via `appSessionMap.get(appId)`
4. Auto-handler creates a **per-prompt subscription** that:
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

### 2.6 Shutdown

1. pi sends `{"type":"exit"}` or pi exits
2. Harness sets `should_exit = true` in the doc
3. Harness kills makepad-host child process and exits

## 3. Shared Document

`AgentDoc` in `shared/src/lib.rs`:

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

## 4. Debug System

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
    // ── WindowClosed early exit ─────────────────────────────────
    // macOS IME null-pointer crash: first_rect_for_character_range
    // fires on the NSView AFTER it's been deallocated by Window close
    // processing. Exit BEFORE any widget processing to avoid the segfault.
    if matches!(event, Event::WindowClosed(_)) {
        doc.should_exit = true;  // signal harness: intentional exit
        std::process::exit(0);   // die before Widget touches the close
    }

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
- **`sync_streaming_text` and `sync_pi_data_to_splash` only run on `Event::Signal`** — not on Draw, Mouse, or Timer events. This prevents reading+hydrating the CRDT doc at 60fps (which caused CPU spikes during streaming).
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

**To click: use orphan coordinates directly — they are already in window-relative space.**

```
click_x = orphan_widget.x + (orphan_widget.width / 2)
click_y = orphan_widget.y + (orphan_widget.height / 2)
```

Do NOT add window position. Do NOT add parent offsets.

**Example from the compact_dump format (W3):**
```
9 -1 - View 26 132 928 105           # orphan View, window-relative (26,132)
10 9 display Label 50 162 22 32      # display at (50,162) in window space
11 9 - Button 442 224 41 22           # Click button at (442,224), center (462,235)
```
Click at window-relative (462, 235) to hit the button at (442, 224, 41, 22).

### Clipped Rect Issue (Critical for Nested Widgets)

Containers with `padding` or `show_bg: true` create draw clips that affect their children's `area.clipped_rect()`. The hit-test for mouse events uses `area.clipped_rect()`, NOT `area.rect()`. If a child widget overflows the parent's padded content area, its `clipped_rect` is reduced to the overlap, potentially making it UNHITTABLE.

**Workaround:** Keep interactive buttons as direct orphans (not wrapped in containers with padding), or ensure they fit within the parent's padded content area.

### First Use Pattern (Standard Interaction Workflow)

1. **Launch**: `launch_makepad_app app_id="my-app" splash_body="..."`
2. **Snapshot**: `check_debug_app debug_command=widget_snapshot debug_params="{}"` — find orphaned widgets at bottom (`"window_id": ""`)
3. **Calculate click center**: orphan widget coordinates ARE window-relative, so use `x + w/2, y + h/2` directly
4. **Click**: `check_debug_app debug_command=click debug_params='{"x":490,"y":185}'`
5. **Verify**: Use `inspect_makepad_doc` to read `user_response` (synchronous, always works) OR `wait_for_response`
6. **For TextInput**: `type_text` fills the first TextInput found in the splash body's widget hierarchy. To verify which input was filled, check the `value` field on orphan TextInputs in `widget_snapshot`.

**CRITICAL: Always take a fresh snapshot before each click** — orphan coordinates shift after layout changes (e.g., adding list items moves buttons down).

**Use `inspect_makepad_doc` for response** — `wait_for_response` may time out if the response arrived before the listener was set up. `inspect_makepad_doc` is synchronous and always reflects the current doc state.

### Known Interaction Issues

- **Coordinates shift after layout changes** — when content grows, orphan coordinates from a previous snapshot become stale. Always take a fresh snapshot before each click if the UI has changed.
- **`type_text` fills the first TextInput within splash children** — walks depth-first, stops at first match. Will not accidentally fill the makepad-host `source` editor.
- **Clicking at coordinates that don't hit any widget** — harmless no-op, no crash, no response sent.
- **Empty string `send_pi_response`** — harmless no-op.

### Rendering Error Handling

When splash body fails to render:
1. Makepad renders dark-red error fallback ("Splash app could not be rendered")
2. `error_message` is written to CRDT doc
3. Harness forwards `{"type":"error","app_id":"...","message":"..."}` to pi
4. The launch tool has a 1.5s debounce window after receiving `status=Launched` to collect any error messages. Errors persist in a `lastErrors` map per app_id.

## 5. Background Sub-Agent Sessions

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

The AgentSplash injects a `__ai_text := TextInput{height:34 width:Fill}` widget that
auto-displays the sub-agent's response — no manual reading needed.

**Streaming architecture (per-prompt subscription):** When the auto-handler processes
an `ai:ask:` message, it creates a **per-prompt subscription**:

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
- `sync_streaming_text()` is called **only on `Event::Signal`** to avoid reading+hydrating the CRDT doc at 60fps
- The `log` widget uses `rfind("\n🤖 ")` (not `rfind('\n')`) to correctly find the AI response boundary when the response contains internal newlines

**Also has a session-level subscription** (`setupSessionStreaming`) that silently
accumulates deltas into `stored.accumulated` (does NOT send to harness). This is
a fallback for `send_background_message` tool usage and future use.

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

### Inline Runsplash Rendering

Runsplash code can be rendered **inline** inside the chat app via a nested AgentSplash
widget injected into every splash body's `SPLASH_SUFFIX`.

**How it works:**
1. `SPLASH_SUFFIX` includes `__run_splash := AgentSplash{width:Fill height:Fit is_root:false}`
2. The nested AgentSplash has `is_root:false`, so it does NOT sync from the CRDT doc
3. During streaming, `sync_streaming_text()` extracts `` ```runsplash `` blocks from
   the accumulated text and calls `run_splash.set_text(cx, &runsplash_code)`
4. The nested AgentSplash evaluates the runsplash code and renders it **inline**
   below the chat app (preserving the chat state)
5. `set_text()` has built-in error recovery: if `eval_body` fails, it restores the
   previous valid body, so incomplete partial code silently keeps the last working UI
6. Log shows "⚙ Generating..." during streaming, "✅ Done" on completion
7. On completion, `sync_pi_data_to_splash()` also runs the runsplash code through
   the nested AgentSplash, replacing any partial rendering with the final result

**Flow:**
1. Sub-agent streams deltas → auto-handler sends `send_streaming_delta` → harness appends to CRDT `streaming_text`
2. CRDT syncs to host → `Event::Signal` fires
3. `sync_streaming_text()` runs (agent_splash.rs, only on Signal):
   - Writes accumulated text to `__ai_text` label
   - Extracts Splash DSL code from the text (handles `` ```runsplash ``, `` ```splash ``, plain `` ``` ``, or raw DSL with no backticks)
   - Calls `__run_splash.set_text(cx, &code)` which evaluates and renders the code inline
4. On streaming completion, `sync_pi_data_to_splash()` writes the final text to `__pi_data` and also re-evaluates through `__run_splash` for the final rendered result

**Known limitations:**
- Nested AgentSplash children are invisible in `widget_snapshot` and `widget_dump` — only the AgentSplash widget itself shows, not its rendered children
- Buttons may overflow Fit height by a few pixels
- AI frequently generates invalid DSL syntax (commas between properties, `id:` instead of `:=`, etc.)
- Sending a second `ai:ask:` while the first is still streaming is handled by `streamingBehavior: "steer"`
- Do not render partial code during streaming (only on completion) to avoid NaN layout crashes from circular `width:Fill` / `height:Fit` dependencies

**Key constraint:** `sync_streaming_text()` and `sync_pi_data_to_splash()` only run on `Event::Signal` (not Draw/Mouse/Timer), to avoid 60fps CRDT doc reads.

**Files:**
- `agent_splash.rs` — `sync_streaming_text()`, `sync_pi_data_to_splash()`, `SPLASH_SUFFIX` injection
- `app.rs` — `exit(0)` on `WindowClosed` to prevent IME dangling pointer crash, `catch_unwind` in `handle_event`
- `harness/src/main.rs` — `child.try_wait()` host death monitor, `panic_backtrace` forwarding
- `.pi/extensions/makepad/dist/doc-bridge.js` — `host_died` WebSocket handler → `disposeAllSessions()`

## 6. Splash DSL Guide

### 6.1 Critical Layout Rules

> **AVOID `padding:Inset{...}` on a `RoundedView` or `View` that contains nested `View{flow:Right}` children.** This causes the inner buttons to become **unhittable** (their `clipped_rect` is reduced by the padded area; the hit-test uses `clipped_rect`, not `area.rect()`).
>
> **Fix:** Use `spacing` between orphan buttons in `flow:Down` (no `View{flow:Right}` wrapper for buttons). Or omit `padding` on the outer container.

```splash
// ✅ SAFE — direct orphans, no padding, no flow:Right wrapper
RoundedView{width:Fill height:Fit flow:Down spacing:12 show_bg:true ...
  display := Label{...}
  ButtonFlat{text:"-"}  // direct orphans are always hittable
  ButtonFlat{text:"+"}
}

// ❌ PROBLEMATIC — padding + flow:Right = buttons are UNHITTABLE (confirmed still broken)
RoundedView{width:Fill height:Fit padding:16 flow:Down ...
  View{flow:Right spacing:12
    ButtonFlat{text:"-"}
    ButtonFlat{text:"+"}
  }
}
```

### 6.2 Key Rules

- **`let`/`fn` declarations must be at the top**, before any widget.
- **Every container MUST have `height: Fit`** — most common failure mode.
- **Root container MUST use `width: Fill`** — never a fixed pixel width.
- `ui` object is built-in; do NOT declare it with `:=`.
- **`for` loops render widgets at build time only** — array changes do NOT re-render. Use `set_text()` for dynamic content.
- **`as int` type casting produces NaN** — use string display + `set_text()` only.
- Every `TextInput` must have a fixed numeric height (e.g. `34`).
- No `on_render` in embedded apps.
- **`widget_tree_mark_dirty` in `render_body` is MANDATORY** — without it, `eval_body` stores the new widget tree but the host never draws it (app stays at loading state). This is already correctly set in the codebase.

### 6.3 Widget Availability

**Available:** View, RoundedView, Label, TextInput, LinkLabel, Button, ButtonFlat, ButtonFlatter, Slider, CheckBox, CheckBoxFlat, RadioButton, RadioButtonFlat, ToggleFlat, DropDown, TabBar, Tab, PopupMenu, ScrollBar, ScrollBars, LoadingSpinner, Hr, Vr, Icon

**NOT available (silently fail):** Stack, Divider, ProgressBar, IconButton, ToggleButton, Image, ListView, Grid, ColorPicker, ScrollPair

| Wanted | Use Instead |
|--------|-------------|
| Divider line | `Hr{height:1 width:Fill}` |
| Progress bar | `Slider{value:0.65 is_read_only:true}` |
| Tabbed UI | `ButtonFlat` rows (TabBar renders zero-size) |

### 6.4 Styling Gotchas

- **`draw_bg.border_radius` takes a float**, not an Inset: `draw_bg.border_radius: 16.0`
- **`#x` prefix for hex colors containing 'e'**: Use `#x2ecc71` (not `#2ecc71`) when the hex contains the letter `e` adjacent to digits, to avoid parser ambiguity with scientific notation.
- **Default text color is white** — for light backgrounds, always set `draw_text.color` on every text element.
- **`new_batch: true` for text visibility** — required on any container with `show_bg: true` that contains text children:
  ```splash
  RoundedView{width:Fill height:Fit new_batch:true show_bg:true draw_bg.color:#x334
    Label{text:"Visible" draw_text.color:#fff}
  }
  ```

### 6.5 Variable Scope

**`let` variables DO persist** across click events (counter, toggle states work correctly).

However, **widget `checked` state** on `RadioButton`, `ToggleFlat`, `CheckBox` does NOT persist because internal post-processing discards the `on_click` scope context. Use `ButtonFlat` with manual toggle for persistent boolean state:

```splash
let toggled = false
ButtonFlat{text:"Toggle" on_click:||{toggled = !toggled; ui.display.set_text("" + toggled)}}
```

### 6.6 Patterns

#### 6.6.1 Struct Arrays & Array Operations

The Splash VM supports arrays of structs with `.push()`, `.remove()`, `.len()`, and `.retain()`. Read fields via `array[index].field`, update with `array[index] += {field: val}`.

**⚠️ `for i in items` iterates over VALUES, not indices.** Use `while` loop with explicit index:

```splash
let idx = 0
while idx < items.len() {
    out = out + items[idx]
    idx = idx + 1
}
```

#### 6.6.2 Component / Template Pattern

```splash
let ItemRow = RoundedView{
    width: F ill height: Fit
    flow: Right spacing: 10
    label := Label{text: "item" width: Fill}
    action := ButtonFlatter{text: "Do" width: 56 height: 28}
}
row_0 := ItemRow{
    label.text: "First item"
    action.on_click: || do_something(0)
}
```

Override syntax: `<child-name>.<property>: <value>` — every segment in the path must use `:=`.

#### 6.6.3 Pre-allocated Fixed Slots

`for` loops render at build-time only — array changes don't add/remove widgets. Pre-allocate a fixed number of rows and update via sync functions:

```splash
let items = [{text: "Item 1"} {text: "Item 2"}]
fn sync_row_0(){
    if 0 < items.len() {
        ui.row_0.label.set_text(items[0].text)
    }
}
```

#### 6.6.4 Numeric State Pattern

```splash
let count = 0
RoundedView{width:Fill height:Fit flow:Down spacing:10 new_batch:true
  display := Label{text:"0" draw_text.color:#x44cc88 draw_text.text_style.font_size:32}
  ButtonFlat{text:"-" on_click:||{count -= 1; ui.display.set_text(count + "")}}
  ButtonFlat{text:"+" on_click:||{count += 1; ui.display.set_text(count + "")}}
}
```

Use `count + ""` to convert numbers to strings.

#### 6.6.5 Dynamic List Display

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

#### 6.6.6 TextInput with on_return

```splash
input := TextInput{
    width: Fill height: 34
    empty_text: "Enter something"
    on_return: |text| add_item(text)
}
Button{text: "Add" width: 64 height: 34 on_click: || add_item(ui.input.text())}
```

### 6.7 Naming Children

| Syntax | Effect |
|--------|--------|
| `label := Label{text:"default"}` | ✅ Addressable via `ui.label`, overridable |
| `label: Label{text:"default"}` | ❌ Static — NOT addressable |

Every segment in an override path must use `:=`

### 6.8 Styling Reference

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

### 6.9 View Scrolling

Use `scroll_bars` as a View property:

```splash
View{width:Fill height:300
  scroll_bars: ScrollBars{show_scroll_x:false show_scroll_y:true
    scroll_bar_y: ScrollBar{drag_scrolling:true}
  }
  ...
}
```

`ScrollEvent` dispatch has no effect — scroll bars only respond to touch/mouse gestures.

## 7. Known Limitations & Still-Broken Issues

| Limitation | Workaround |
|-----------|------------|
| `debug_response` may arrive repeatedly | Accept first response, ignore duplicates |
| `pending_click` is a single slot — two rapid clicks overwrite | Take a fresh `widget_snapshot` between clicks |
| `wait_for_response` may time out | Use `inspect_makepad_doc` (synchronous) instead |
| Widget text shows `" "` (space) instead of `""` for `__pi_response` | Use `value` field for TextInput, not `text` field |
| Stale content after rapid close+launch | Wait 1-2 seconds between close and launch |
| Debug commands freeze after ~50 ops | Kill both processes, rebuild, restart |
| Coordinates shift after layout changes | Always take a fresh `widget_snapshot` before each click |
| Container padding clips children's hit areas | Keep buttons as direct orphans (no container wrapping) |
| Background sub-agent slow (5-20s API call) | Wait for response; check harness logs |
| `__ai_text` is a TextInput — fills before user's in `type_text` | Put user's TextInput FIRST in splash body (default is correct) |
| Sub-agent session dispose warning | Call `stop_background_session` when done |
| `for i in items` iterates over values (not indices) in Splash VM | Use `while idx < items.len()` with `items[idx]` for correct indexing |
| `while` loops in Splash can cause debug system timeouts | Allow 10s+ cooldown after using `while` in `on_click` |

### ACTIVE BUG: `dy.is_nan()` crash in `move_align_list` (turtle.rs:2342)

**Status: NEWLY REPRODUCED (2026-07-14)** — crashes on initial render during `DrawQuad::end()`, then **re-crashes on every subsequent Draw event** (persistent crash loop). Caught by `catch_unwind` but the app is effectively dead — `close_makepad_app` can't recover it because the host panics on every event cycle. **Recovery requires `pkill -f makepad-host`.**

**Trigger conditions (all must be present):**
- A `View{flow:Right}` containing **3+ siblings** (buttons, labels, etc.)
- A **separator widget** between two flow:Right groups (Hr, Label, View{height:1 width:Fill} — any widget with a fixed height that separates them)
- At least one more `View{flow:Right}` (any size) after the separator
- **Padding is NOT required** — crash reproduces with no padding, no `show_bg:true`

The crash is triggered by the layout arithmetic combining a multi-line flow:Right group, a fixed-height separator child, and another flow:Right group. The `move_align_list` alignment calculation produces a NaN delta-y.

**Redacted backtrace (useful frames only):**
```
assertion failed: !dy.is_nan()
  at Cx2d::move_align_list  (turtle.rs:2342)
  → Cx2d::end_turtle_with_guard  (turtle.rs:1635)
  → Cx2d::end_turtle  (turtle.rs:1464)
  → DrawQuad::end  (draw_quad.rs:122)
  → View::draw_walk  (view.rs:965)
  → AgentSplash::draw_walk  (agent_splash.rs:389)
  → catch_unwind  (app.rs:509)   ← host survives but re-crashes on next Draw
```

**Confirmed safe (does NOT crash):**
- `View{flow:Right}` with only 2 siblings (then separator, then another) ✓
- Two `View{flow:Right}` groups adjacent with no separator between them ✓
- Separator before any flow:Right groups (at the top) ✓
- A single `View{flow:Right}` group followed by a separator (no second group) ✓
- Direct orphan buttons in `flow:Down` (no `flow:Right` wrapper) ✓

**What does NOT work (do not try):**
- Removing padding — crash is NOT about padding, padding is irrelevant
- Replacing Hr with a Label separator — same crash (it's ANY separator, not specifically Hr)
- Replacing Hr with a View{height:1} — same crash
- Adding `width:Fill` — already present, crash still happens
- Manipulating injected widgets (Label/TextInput/__ai_text) — the crash is in splash body layout, not injected widgets

**What DOES work (avoid the trigger):**
- Use at most 2 siblings in any `View{flow:Right}` when a separator follows
- Place the separator at the start (before all flow:Right groups), not between them
- Remove the separator between flow:Right groups (use `spacing` on the parent instead)
- Use direct orphan buttons in `flow:Down` with `spacing` instead of `View{flow:Right}` wrappers

### ACTIVE BUG: Buttons inside `padding` + `View{flow:Right}` are UNHITTABLE

**Status: CONFIRMED STILL BROKEN** — tested with end-to-end click simulation, clicks do NOT register. Not a crash, the buttons simply don't respond.

**Root cause:** Containers with `padding` create a draw clip. Makepad's hit-test uses `area.clipped_rect()`, not `area.rect()`. Nested buttons inside `View{flow:Right}` inside a padded container get their `clipped_rect` reduced to the padded content area, so clicks near the padded edge miss entirely.

**What does NOT work (do not try):**
- `cursor: Hand` on inner View — irrelevant for hit-testing
- `size` props on inner View — doesn't change clipped_rect hit-test behavior
- Changing padding on inner View — the outer container's clip limits everything inside
- Using `margin` instead of `padding` — hit-test still uses clipped_rect

**What DOES work:**
- Direct orphan buttons in `flow:Down` with `spacing` — always hittable
- Omitting `padding` on the outer container — no clip created
- Keeping buttons outside any `padding`-ed container's content area

### Host Process Death Recovery

If tools time out or return stale data:
1. Validate with `ps aux | grep makepad-host` or `inspect_makepad_doc` for `panic_backtrace`
2. Harness detects host death via `child.try_wait()` in bridge loop → sends `{"type":"host_died"}` to extension
3. Extension calls `disposeAllSessions()` to clean up background agent sessions
4. Restart by launching a new app (spawns fresh harness + host)

**Recovery from debug freeze:**
```bash
pkill -f makepad-host; pkill -f harness
cargo build -p harness -p makepad-host
```
Then launch a new app.

### What Has Been Fixed (no longer a concern)

These were problems historically. Do NOT attempt to fix them again:

| Fixed Issue | Fix | Verified |
|-------------|-----|----------|
| Streaming-time `dy.is_nan()` crash (text writes to `__ai_text` at 60fps with TextInput{height:0}) | 1. `SPLASH_PREFIX` has `width:Fill` 2. `__ai_text` uses `Label{height:Fit}` not `TextInput{height:0}` 3. `widget_tree_mark_dirty` in `render_body` 4. Signal-only sync (no 60fps doc reads) | Verified during sub-agent streaming + unsafe layout (padding+flow:Right) |
| macOS IME null pointer crash on window close | `exit(0)` at top of `handle_event` for `WindowClosed` (before any widget processing) | Reproduced and confirmed by removing the fix — window close with active TextInput causes deterministic segfault |
| Rust harness `catch_unwind` can't catch IME null pointer | Fixed by `exit(0)` before any widget processing on WindowClosed | Confirmed — IME query runs after widget focus changes, so exiting early prevents it |

### IME Crash Root Cause (macOS)

**Crash chain (without the fix):**

```
1. User clicks red ✕ on window title bar
2. macOS calls window_should_close → cw.send_window_close_requested_event()
3. macOS calls window_will_close → cw.send_window_closed_event()
4. Makepad event loop picks up Event::WindowClosed
5. handle_event → ui.handle_event() → Root → Window processes WindowClosed
6. Window widget tells macOS to close → NSView gets deallocated
7. macOS IME cleanup fires first_rect_for_character_range on the NSView
8. get_cocoa_window(this) reads this.get_ivar("macos_window_ptr")
   → `this` is a DANGLING POINTER (NSView already freed in step 6)
   → SEGFAULT
```

**Why `catch_unwind` can't help:** This is a segfault (dangling pointer dereference), not a Rust panic. `catch_unwind` only catches Rust panics. The crash is outside Rust's control — it's in Objective-C message dispatch (`msg_send![view, frame]`) on a freed object.

**How the fix works:** The fix places the entire `WindowClosed` check **before** `catch_unwind` and **before** `ui.handle_event()`. When `WindowClosed` arrives:

1. Write `should_exit = true` to CRDT doc (so the harness knows the exit was intentional, not a crash)
2. Call `std::process::exit(0)` — terminates the process immediately

Because `exit(0)` runs before `ui.handle_event()`, the Window widget **never processes the close**. The NSView is never deallocated during event processing. The IME delegate never gets a chance to query the dangling pointer. The process just dies cleanly.

**The CRDT `should_exit` write is best-effort** — it races with `exit(0)`, but since the CRDT is in the harness process (in-memory), the write goes through before the host dies. This prevents the harness from firing `host_died` → `disposeAllSessions()` unnecessarily.

**Is this a general Makepad bug?** Yes — the crash pathway is the same in any Makepad app on macOS. A stock `cargo makepad new` app with a focused TextInput would hit the same crash on window close. The fix is specific to our `handle_event` because we have a custom event loop with doc sync and debug dispatch. Most Makepad apps don't have this code, but they still have the same underlying macOS IME + NSView deallocation race.

## 8. Build, Test, Logs

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
