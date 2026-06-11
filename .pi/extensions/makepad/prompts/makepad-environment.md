You are operating in a Makepad mini-app environment.

## Splash Body Structure

State variables (`let x = ...`) MUST be at the TOP of the body, before any widget:

```splash
let count = 0     ← correct
RoundedView{...}  ← root widget after all let/fn declarations
```

The body is the content of an outer View (hidden from you). Start with `let` and `fn` declarations, then the root widget.

## Container Rules (Critical!)

### 1. Every container MUST have `height: Fit`

**Every container — `View`, `RoundedView` etc — MUST have `height: Fit`:**

```
✅ RoundedView{width:Fill height:Fit flow:Down padding:16 ...}
❌ RoundedView{width:Fill flow:Down padding:16 ...}  ← invisible!
❌ View{padding:30 ...}  ← invisible without height:Fit!
```

Exception: Inside a fixed-height parent, `height: Fill` is fine.

### 2. Root container MUST use `width: Fill`

**NEVER use a fixed pixel width (e.g., `width: 400`) on the outermost container.** Your output renders inside a container that provides available width — use `width: Fill` to fill it.

```
✅ RoundedView{width:Fill height:Fit flow:Down ...}
❌ RoundedView{width:400 height:Fit flow:Down ...}  ← narrow sliver or clipped!
```

A fixed width does not adapt to available space. If the parent is narrower, content gets clipped.

### 3. `draw_bg.border_radius` takes a float, not an Inset

```
✅ draw_bg.border_radius: 16.0
❌ draw_bg.border_radius: Inset{top:0 bottom:16 left:0 right:0}  ← parse error!
```

`border_radius` is a single `f32` value applied uniformly to all corners. Passing an `Inset` will silently break your entire layout.

## Label Styling

Both syntaxes work for Label text styling:

```splash
Label{text:"Hello" color:#x2ecc71 font_size:16 height:24}          ← bare props work
Label{text:"Hello" draw_text.color:#x2eccyr draw_text.text_style.font_size:16}  ← draw_text also works
```

All three properties `color:`, `font_size:`, `font_weight:` work directly. You can use either style.

### ⚠️ Default text color is WHITE

**All text widgets (Label, H1–H4, Button text, etc.) default to white (`#fff`).** For light/white themes, you MUST explicitly set `draw_text.color` to a dark color on EVERY text element, or text will be invisible (white-on-white):

```
RoundedView{draw_bg.color:#f5f5f5 height:Fit new_batch:true
  Label{text:"Visible!" draw_text.color:#222}  ← dark text required for light bg
}
```

### ⚠️ Use `#x` prefix for hex colors containing 'e'

When a hex color contains the letter `e` adjacent to digits (like `#1e1e2e`), use the `#x` prefix to avoid parser ambiguity:

```
#x2ecc71    ← contains 'e' next to digits, use #x
#x1e1e2e    ← contains 'e' next to digits, use #x
#ff4444     ← no 'e' issue, plain # works
#00ff00     ← no 'e' issue
```

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

### ⚠️ Naming children with `:=` — the `:=` operator

Children you want to reference or override later MUST use the `:=` operator (not `:`):

```
label := Label{text:"default"}    ← ✅ named/dynamic child, addressable via ui.label
label: Label{text:"default"}     ← ⛔ static child, NOT addressable, overrides fail silently
```

### ⚠️ Named children inside anonymous containers are UNREACHABLE

If a `:=` child is nested inside an anonymous `View{}` (no `:=` on the View), the override path cannot find it. Every container in the path must also have a `:=` name:

```
// ❌ label inside unnamed View is unreachable
let Item = View{flow:Right
  View{flow:Down
    label := Label{text:"default"}  ← UNREACHABLE: parent View is anonymous
  }
}
Item{label.text:"new text"}  ← silent failure, shows "default"

// ✅ Give every container in the path a := name
let Item = View{flow:Right
  texts := View{flow:Down
    label := Label{text:"default"}  ← reachable via texts.label
  }
}
Item{texts.label.text:"new text"}  ← works!
```

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
| `on_render` in dynamic lists | ❌ Blocked by validator | Use pre-allocated named widgets + sync functions instead |
| `ToggleButton` | ❌ Doesn't exist | Use `ToggleFlat` instead |
| `set_text()` triggering `on_return` | ❌ Bypassed | `type_text` debug sets TextInput value directly but does NOT fire `on_return` callback |

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

## Draw Batching: `new_batch: true`

**Set `new_batch: true` on any View/RoundedView with `show_bg: true` that contains text children.** Makepad batches same-shader widgets into one GPU draw call. Without `new_batch: true`, text renders **behind** backgrounds (invisible text).

```
// ✅ Correct: new_batch prevents text from rendering behind background
RoundedView{
  width:Fill height:Fit
  new_batch:true
  show_bg:true
  draw_bg.color:#334
  Label{text:"This text is visible" draw_text.color:#fff}
}

// ❌ Wrong: text may be invisible (draws behind bg)
RoundedView{
  width:Fill height:Fit
  show_bg:true
  draw_bg.color:#334
  Label{text:"Invisible text!" draw_text.color:#fff}
}
```

**When to use `new_batch: true`:**
- Any container with `show_bg: true` that contains Labels or other text
- The **parent container** of children that have their own backgrounds (repeated list items)
- Hoverable items — without it, text vanishes on hover when background becomes opaque

## What NOT to Use

- `on_render` — blocked by the validator (pre-allocated named widgets + `sync()` functions work instead)
- `fn` at the top level that's not a function declaration — the root must be a widget tree
- Parenthesized `if` conditions — use `if cond { ... }` syntax
- `on_click` on a `View` or non-button widget — only button types (`Button`, `ButtonFlat`, `ButtonFlatter`) have `on_click`

## Standard Apps

Pass `standard_app: "<name>"` to `launch_makepad_app` for built-in templates:

| App | Description |
|-----|-------------|
| `counter` | Increment/decrement/reset counter |
| `notes` | Quick notes with add, delete, clear-all (5 slots) |
| `todo` | Task list with add, toggle, delete, clear-completed (5 slots) |

**Note:** There is no timer standard app — `set_interval()` is not available in this Makepad build.

## Example: Pre-Allocated Dynamic List (since `on_render` is blocked)

For dynamic lists where the max size is known, use pre-allocated named widgets and a `sync()` function:

```splash
let items = ["Apple" "Banana" "Cherry"]
let max_items = 5

fn sync_all(){
  sync_row_0()
  sync_row_1()
  sync_row_2()
  sync_row_3()
  sync_row_4()
  ui.status.set_text(items.len() + " items")
}

fn sync_row_0(){
  if 0 < items.len() { ui.row0.set_text(items[0]) }
  else { ui.row0.set_text("Empty slot") }
}
fn sync_row_1(){
  if 1 < items.len() { ui.row1.set_text(items[1]) }
  else { ui.row1.set_text("Empty slot") }
}
fn sync_row_2(){
  if 2 < items.len() { ui.row2.set_text(items[2]) }
  else { ui.row2.set_text("Empty slot") }
}
fn sync_row_3(){
  if 3 < items.len() { ui.row3.set_text(items[3]) }
  else { ui.row3.set_text("Empty slot") }
}
fn sync_row_4(){
  if 4 < items.len() { ui.row4.set_text(items[4]) }
  else { ui.row4.set_text("Empty slot") }
}

fn add_item(text){
  if text == "" { return }
  if items.len() >= max_items { return }
  items.push(text)
  ui.input.set_text("")
  sync_all()
}

RoundedView{width:Fill height:Fit flow:Down spacing:10 padding:16 new_batch:true draw_bg.color:#x1e1e2e draw_bg.border_radius:10.0
  Label{text:"Items" draw_text.color:#fff}
  View{flow:Right spacing:8
    input := TextInput{width:Fill height:34 empty_text:"Add item" on_return:|text| add_item(text)}
    ButtonFlat{text:"Add" on_click:|| add_item(ui.input.text())}
  }
  row0 := Label{text:"Apple" draw_text.color:#ddd}
  row1 := Label{text:"Banana" draw_text.color:#ddd}
  row2 := Label{text:"Cherry" draw_text.color:#ddd}
  row3 := Label{text:"Empty slot" draw_text.color:#888}
  row4 := Label{text:"Empty slot" draw_text.color:#888}
  status := Label{text:"3 items" draw_text.color:#888}
}
```

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

## Debug Commands (`check_debug_app`)

### Splash Content Orphan Issue

Splash content widgets (the inner View created by evaluating the body) have `parent = -1` in the widget tree. This means:
- `widget_id` lookups via `find_within()` **will NOT find splash content widgets**
- **Use coordinate-based clicks** from `widget_dump` or `widget_snapshot` output
- `type_text` works because it walks `try_children()` directly (bypassing the widget tree)
- `widget_snapshot` and `widget_dump` DO include orphaned widgets in their output

### Important Click Behavior

- Click coordinates from the dump/snapshot are **window-content-relative**
- Calculate center as `x + w/2, y + h/2`
- Setting `widget_id` for click lookup does NOT work for splash content
- `type_text` writes directly to the TextInput but does NOT trigger `on_return` callbacks — you need to click a button that reads the value to process it

## Host Process Awareness

- The makepad-host process can crash independently of the harness (bridge process)
- `list_makepad_apps` shows the last stored state, which may be stale if the host crashed
- If debug commands time out, the host may have crashed — try launching a fresh app
- The harness does NOT currently detect host crashes automatically

## Tools

- `launch_makepad_app(app_id, splash_body, standard_app?)` — Launch/replace a mini-app
- `close_makepad_app(app_id)` — Close the running app
- `list_makepad_apps()` — List the running app and any error
- `check_debug_app(app_id?, retry_splash_body?, debug_command?, debug_params?, timeout_seconds?)` — Inspect the widget tree, check errors, retry with a fix, or simulate interactions
- `store_value(key, value, description)` — Persist a value
- `read_value(key)` — Read a stored value
