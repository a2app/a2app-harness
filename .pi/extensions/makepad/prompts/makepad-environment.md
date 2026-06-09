You are operating in a Makepad mini-app environment.

## Splash Body Structure

State variables (`let x = ...`) MUST be at the TOP of the body, before any widget:

```splash
let count = 0     ← correct
RoundedView{...}  ← root widget after all let/fn declarations
```

The body is the content of an outer View (hidden from you). Start with `let` and `fn` declarations, then the root widget.

## Container Rules (Critical!)

**Every container — `View`, `RoundedView` etc — MUST have `height: Fit`:**

```
✅ RoundedView{width:Fill height:Fit flow:Down padding:16 ...}
❌ RoundedView{width:Fill flow:Down padding:16 ...}  ← invisible!
❌ View{padding:30 ...}  ← invisible without height:Fit!
```

Exception: Inside a fixed-height parent, `height: Fill` is fine.

## Label Styling

Both syntaxes work for Label text styling:

```splash
Label{text:"Hello" color:#x2ecc71 font_size:16 height:24}          ← bare props work
Label{text:"Hello" draw_text.color:#x2eccyr draw_text.text_style.font_size:16}  ← draw_text also works
```

All three properties `color:`, `font_size:`, `font_weight:` work directly. You can use either style.

## Interactive Callbacks

```splash
display := Label{text:"0" draw_text.color:#x44cc88 draw_text.text_style.font_size:32}
ButtonFlat{text:"+" on_click:||{count += 1; ui.display.set_text(count + "")}}
```

- `ui.<name>` is built-in — no `:=` declaration needed for `ui` itself
- Concatenate with `+ ""` to convert numbers: `count + ""`
- Functions (`fn my_fn(){...}`) can use `ui.<name>` too
- Use `;` to separate multiple statements in closures: `{count += 1; ui.label.set_text(count + "")}`
- `ui.<name>.text()` reads TextInput content
- `ui.<name>.set_text("...")` updates any widget's text content
- **Every container** — including inner `View{flow:Right}` — MUST have explicit `height: Fit`

## Sending Data Back to Pi

Splash apps can send data back to the pi extension by setting text on the built-in `__pi_response` label:

```splash
ButtonFlat{text:"Send" on_click:||{ui.__pi_response.set_text("hello from splash")}}
ButtonFlat{text:"Send Count" on_click:||{count += 1; ui.__pi_response.set_text("count: " + count)}}
```

The `__pi_response` label is automatically available in every splash app (injected by the AgentSplash wrapper). Setting its text writes to the shared CRDT document's `user_response` field, which the harness forwards to pi as a `{"type":"user_response",...}` JSON message.

Each new non-empty text triggers a new response. Empty strings are ignored.

## What Does NOT Work (Verified)

| Feature | Status | Notes |
|---------|--------|-------|
| `set_interval()` / `clear_interval()` | ❌ Not available | Cannot create timers or countdowns |
| `send_response()` from splash body | ❌ Not callable | Only callable from parent app code |
| `Stack` widget | ❌ Doesn't exist | Use manual positioning |
| `Divider`, `ProgressBar`, `IconButton` | ❌ Don't exist | Use `Hr` for dividers |

## Known Limitation: Colons in String Arguments

**Avoid colons (`:`) inside string arguments passed to `ui.*.set_text()` calls.** The pre-validation parser incorrectly extracts the part before the first colon on each line, so this triggers a false error:

```
❌ ui.display.set_text("1:00")       ← colon inside string argument triggers false positive
✅ ui.display.set_text("1" + ":" + "00")  ← workaround
```

This is a known bug in the compiled validator. It will be fixed when the pi process restarts with updated code.

## Widget Availability

These widgets are available in this Makepad build. All others will fail silently (not render):

| Category | Widgets |
|----------|---------|
| **Containers** | `View`, `RoundedView` |
| **Text** | `Label`, `TextInput`, `LinkLabel` |
| **Buttons** | `Button`, `ButtonFlat`, `ButtonFlatter` |
| **Inputs** | `Slider`, `CheckBox`, `CheckBoxFlat`, `RadioButton`, `RadioButtonFlat`, `ToggleFlat` |
| **Menus/Lists** | `DropDown`, `TabBar`, `Tab`, `PopupMenu`, `ScrollBar`, `ScrollBars`, `LoadingSpinner` |
| **Decorations** | `Hr`, `Vr`, `Icon` |

**NOT available:** `Stack`, `Divider`, `ProgressBar`, `IconButton`, `ToggleButton`, `Image`, `ListView`, `Grid`, `ColorPicker`, `ScrollPair`

## TextInput Requirements

Every `TextInput` must use a fixed numeric height:

```
✅ TextInput{height:34 hint:"Enter name"}
❌ TextInput{height:Fit hint:"Enter name"}
❌ TextInput{height:Fill hint:"Enter name"}
```

## What NOT to Use

- `on_render` — destabilizes embedded apps
- `fn` at the top level that's not a function declaration — the root must be a widget tree
- Parenthesized `if` conditions — use `if cond { ... }` syntax

## Standard Apps

Pass `standard_app: "<name>"` to `launch_makepad_app` for built-in templates:

| App | Description |
|-----|-------------|
| `counter` | Increment/decrement/reset counter |
| `notes` | Quick notes with add, delete, clear-all (5 slots) |
| `todo` | Task list with add, toggle, delete, clear-completed (5 slots) |

**Note:** There is no timer standard app — `set_interval()` is not available in this Makepad build.

## Example: Interactive Counter

```splash
let count = 0
RoundedView{width:Fill height:Fit flow:Down spacing:10 padding:16 new_batch:true draw_bg.color:#x1e1e2e draw_bg.border_radius:10.0
  Label{text:"Counter" draw_text.color:#fff draw_text.text_style.font_size:16}
  display := Label{text:"0" draw_text.color:#x44cc88 draw_text.text_style.font_size:32}
  View{flow:Right spacing:12 align:Align{x:0.5 y:0.5}
    ButtonFlat{text:"-" on_click:||{count -= 1; ui.display.set_text(count + "")}}
    ButtonFlat{text:"Reset" on_click:||{count = 0; ui.display.set_text("0")}}
    ButtonFlat{text:"+" on_click:||{count += 1; ui.display.set_text(count + "")}}
  }
}
```

## Tools

- `launch_makepad_app(app_id, splash_body, standard_app?)` — Launch/replace a mini-app
- `close_makepad_app(app_id)` — Close the running app
- `list_makepad_apps()` — List the running app and any error
- `check_debug_app(app_id?, retry_splash_body?)` — Check errors or retry with a fix
- `store_value(key, value, description)` — Persist a value
- `read_value(key)` — Read a stored value
