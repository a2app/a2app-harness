# A2App: Agentic App Building and Runtime System

A2App is a system for building and running agentic applications. It composes an inference endpoint, a shared workspace, an app runtime, and a coordination harness into a coherent whole.

---

## Key Features

Five capabilities define the system, independent of any particular implementation:

<img src="a2app-features.svg" alt="A2App key features" width="700"/>

---

## Reference Implementation

One concrete way to implement A2App maps the four features onto three components:

<img src="a2app-components.svg" alt="A2App components" width="720"/>

---

## Component Notes

**A — Coding Agent** handles inference (feature 1), workspace access (feature 2), and the main agent chat UI (feature 4). It runs the LLM loop, provides the user-facing chat interface, reads and writes the filesystem, and communicates outward via a WebSocket connection to the Rust process.

**B — Rust Process** is the harness (feature 5). It runs a WebSocket server that the coding agent connects to, and maintains a CRDT for state synchronisation with the app runtime. The CRDT could in principle live in the coding agent, but cross-language friction made a WebSocket bridge the pragmatic call.

**C — App Runtime** covers the runtime (feature 3). It hosts a Makepad application shell capable of launching Splash mini-apps, and consumes state from the Rust process via the CRDT.

---

## Tools and Capabilities

The system has three components: a **coding agent** (pi), a **harness** (Rust bridge process), and a **host** (Makepad app runtime). Communication flows: pi ↔ harness (JSON WebSocket, port 2341) and harness ↔ host (samod CRDT sync, port 2342). The CRDT is an implementation detail used to keep the host's UI state in sync with the harness — all messages between pi and the host pass through the CRDT document (`AgentDoc`), but pi never interacts with the CRDT directly.

<a name="tool-launch"></a>
### `launch_makepad_app` (Pi Agent → Harness → Host)

Sends a `{"type": "launch"}` message over JSON WS to the harness, which writes the splash body into the CRDT `pending_app` field. The host receives the CRDT update via samod sync, evaluates the Splash DSL, and renders it in an `AgentSplash` widget. The CRDT here acts as a write-once command queue: the harness writes, the host reads and processes, then clears the field.

<a name="tool-launch-agent"></a>
### `launch_app_with_agent` (Pi Agent → Harness → Host + Sub-Agent)

Like the above but also creates a sub-agent session in the pi extension. The splash body calls `ui.__pi_response.set_text("ai:ask:" + message)` which writes to the CRDT `user_response` field via the host. The harness detects the version increment and forwards the response to pi over JSON WS. The pi extension's auto-handler routes it to the sub-agent. When the sub-agent responds, streaming deltas are sent back through the harness (as `send_streaming_delta` → CRDT `streaming_text`) and the host's `sync_streaming_text()` method updates the UI on `Event::Signal`.

<a name="tool-check-debug"></a>
### `check_debug_app` (Pi Agent → Harness → Host)

Sends a `{"type": "debug"}` message over JSON WS. The harness writes `debug_command` to the CRDT doc. The host's `process_debug_commands()` executes the command and writes the result back to `debug_response`, which the harness forwards to pi. For click/type_text interactions, the harness also sets a `pending_interaction` flag so the bridge loop waits for the host to process before reading stale `user_response` values.

| Command | Executed By | Returns |
|---------|-------------|--------|
| `widget_snapshot` | Host reads `cx.widget_tree()` | JSON array of widgets (id, type, position, size, text, value) |
| `click` | Host dispatches synthetic MouseDown/MouseUp to `splash.handle_event()` | Debug response after execution |
| `type_text` | Host walks splash children depth-first, fills first TextInput | Debug response after execution |

<a name="tool-inspect-doc"></a>
### `inspect_makepad_doc` (Pi Agent → Harness)

Sends a `{"type": "get_doc"}` message over JSON WS. The harness reads the current CRDT `AgentDoc` state directly and returns `app_id`, `user_response`, `error_message`, and `status`. This is synchronous — no CRDT sync needed because the harness owns the doc.

<a name="cap-pi-response"></a>
### `__pi_response` / `__pi_data` (Injected Widgets in Host)

The host's `AgentSplash` injects these widgets into every splash body via `SPLASH_PREFIX` / `SPLASH_SUFFIX`:

- `__pi_response := Label{text:""}` — the splash app calls `set_text()` on this Label to send data back to pi. The host detects the text change in `handle_event()` (on every event type), writes it to the CRDT `user_response` field, and increments `user_response_version` (to detect same-value re-sends like toggles). The harness bridge loop compares version numbers and forwards the response to pi over JSON WS.

- `__pi_data := Label{text:" "}` — the splash app reads `text()` from this Label, which gets its value from the CRDT `pi_response` field. Written by the harness when pi sends `{"type": "send_pi_response"}`, synced to the host on the next `Event::Signal`.

- `__ai_text := Label{text:" "}` — auto-displays sub-agent responses. Updated by `sync_streaming_text()` from the CRDT `streaming_text` field (live streaming deltas) and `sync_pi_data_to_splash()` from `pi_response` (final response).

- `__run_splash := AgentSplash{is_root:false}` — a nested AgentSplash that evaluates and renders `\`\`\`runsplash` code blocks inline. Called by `sync_streaming_text()` during streaming and `sync_pi_data_to_splash()` on completion. Has built-in error recovery: if `eval_body` fails, `set_text()` restores the previous valid body.

<a name="cap-streaming"></a>
### Streaming Deltas (Sub-Agent → Pi Extension → Harness CRDT → Host)

When a sub-agent responds to an `ai:ask:` prompt:

1. The pi extension's per-prompt subscription captures each `text_delta` event
2. Each delta (raw new characters) is sent to the harness as `{"type": "send_streaming_delta"}`
3. The harness **appends** the delta to the CRDT `streaming_text` field
4. The CRDT syncs to the host via samod WebSocket
5. On receiving `Event::Signal`, the host calls `sync_streaming_text()` which:
   - Compares `self.last_streaming_text` with the current CRDT value
   - If changed: updates `__ai_text` Label and extracts `\`\`\`runsplash` code blocks
   - Passes extracted code to `__run_splash.set_text()` for inline rendering (error recovery on partial code)
6. On completion, the extension sends `{"type": "send_streaming_end"}`; the harness sets `pi_response` to the final text and clears `streaming_text`

Key design: CRDT reads only happen on `Event::Signal` (not on 60fps Draw/Mouse events) to avoid CPU jank.

---

## Session: Progressive Splash App Testing (2026-07-09)

Five splash apps launched and tested, from simple to meta. Full session log at [app_gen.jsonl](https://huggingface.co/datasets/gterzian/a2app/blob/main/app_gen.jsonl).

<img src="artifacts/counter app.png" alt="Counter" width="400"/>

**1. Counter** — `counter-simple`

Launched with [`launch_makepad_app`](#tool-launch). Interactive +/- buttons using `let count = 0` variable persistence. Clicked with [`check_debug_app`](#tool-check-debug)`(debug_command="click")` using window-relative orphan coordinates from [`widget_snapshot`](#tool-check-debug). "Send to Pi" button demonstrated splash→pi communication via [`__pi_response.set_text()`](#cap-pi-response), verified with [`inspect_makepad_doc`](#tool-inspect-doc).

---

<img src="artifacts/todo app.png" alt="Todo" width="400"/>

**2. Todo List** — `todo-1`

Launched with [`launch_makepad_app`](#tool-launch). Items added by typing into a TextInput via [`check_debug_app`](#tool-check-debug)`(debug_command="type_text")` then clicking "Add". Uses `while` loop over a struct array with `items.push()`/`items.remove()` for list management. "Remove Last" removes items, "Send to Pi" returns the full list via [`__pi_response`](#cap-pi-response).

---

<img src="artifacts/ai chat app.png" alt="AI Chat" width="400"/>

**3. AI Chat** — `chat-ai-1`

Launched with [`launch_app_with_agent`](#tool-launch-agent) (creates a blank-slate DeepSeek V4 Flash sub-agent). Splash body sends [`__pi_response.set_text("ai:ask:" + msg)`](#cap-pi-response) which the extension auto-handler routes to the sub-agent. Response streams token-by-token into the injected [`__ai_text`](#cap-pi-response) widget via the [streaming delta system](#cap-streaming). Asked "What is a CRDT?" — got a full streaming response with bullet points.

---

<img src="artifacts/splash gen app.png" alt="Splash Generator" width="400"/>

**4. 🌟 Splash Generator** — `splash-gen-1`

Launched with [`launch_app_with_agent`](#tool-launch-agent) using a system prompt teaching correct Splash DSL syntax (`:=` naming, `on_click:||{}`, `width:Fill`, no commas). Asked for "a simple counter with + and - buttons". The AI generated valid `` \`\`\`runsplash `` code which was automatically picked up by [`sync_streaming_text()`](#cap-streaming) and rendered inline via the nested [`__run_splash`](#cap-pi-response) AgentSplash — height grew from 0 to 286px.