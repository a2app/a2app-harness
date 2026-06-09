// Known Splash-compatible widget tags (verified against Makepad widgets/src/)
// Non-Base variants only - check agent_splash.rs for runtime support.
const KNOWN_WIDGETS = new Set([
  // Core containers — VERIFIED working
  "View",
  "RoundedView",
  
  // Labels & text — VERIFIED working
  "Label",
  "TextInput",
  "LinkLabel",
  
  // Buttons — VERIFIED working
  "Button",
  "ButtonFlat",
  "ButtonFlatter",
  
  // Inputs
  "Slider",           // VERIFIED working
  "CheckBox",
  "CheckBoxFlat",
  "RadioButton",
  "RadioButtonFlat",
  "ToggleFlat",
  
  // Lists & menus
  "DropDown",
  "TabBar",
  "Tab",
  "PopupMenu",
  "ScrollBar",
  "ScrollBars",
  "LoadingSpinner",
  
  // Decorations
  "Hr",
  "Vr",
  "Icon",
]);

// Additional notes for agent guidance (not validation):
// - All containers (View, RoundedView) MUST have explicit height:Fit to render
// - Use draw_text.color: and draw_text.text_style.font_size: for Label styling
// - Stack, Divider, ProgressBar, IconButton, ToggleButton, Image, ListView, Grid, ColorPicker are NOT available in this build

// Known Splash DSL property roots (not named widget references)
const PROPERTY_ROOTS = new Set([
  "align",
  "body",
  "content",
  "draw_bg",
  "draw_cursor",
  "draw_icon",
  "draw_selection",
  "draw_text",
  "header",
  "icon_walk",
  "label_align",
  "label_walk",
  "popup_menu",
  "scroll_bar",
  "scroll_bars",
  "walk",
  "window",
  "text",
  "font_size",
  "font_weight",
  "width",
  "height",
  "flow",
  "spacing",
  "padding",
  "margin",
  "new_batch",
  "empty_text",
  "on_return",
  "on_click",
  "on_change",
  "on_close",
  "cursor",
  "line_height",
  "text_style",
]);

/** Check if a line looks like it's defining or invoking an unknown widget */
function looksLikeUnknownWidget(line: string): string | null {
  const trimmed = line.trim();
  if (!trimmed || trimmed.startsWith("//") || trimmed.startsWith("let ") || trimmed.startsWith("fn ")) {
    return null;
  }

  // Match patterns like `WidgetName{...}` or `WidgetName {`
  const widgetMatch = trimmed.match(/^([A-Z][a-zA-Z0-9_]*)\s*\{/);
  if (widgetMatch) {
    const name = widgetMatch[1];
    if (!KNOWN_WIDGETS.has(name)) {
      return `it used unknown widget '${name}' — Splash DSL supports: ${Array.from(KNOWN_WIDGETS).sort().join(", ")}`;
    }
  }

  // Match `widget_ref := WidgetName{`
  const namedMatch = trimmed.match(/^[a-z_]\w*\s*:=\s*([A-Z][a-zA-Z0-9_]*)\s*\{/);
  if (namedMatch) {
    const name = namedMatch[1];
    if (!KNOWN_WIDGETS.has(name)) {
      return `it used unknown widget '${name}' — Splash DSL supports: ${Array.from(KNOWN_WIDGETS).sort().join(", ")}`;
    }
  }

  return null;
}

/** Detect multiline string literals (strings spanning >1 line with actual newlines) */
function hasMultilineStringLiteral(body: string): boolean {
  const lines = body.split("\n");
  let inString = false;
  let quoteChar = '"';
  
  for (const line of lines) {
    if (!inString) {
      // Look for a line that starts a string and doesn't end it on the same line
      const startIdx = line.indexOf('"');
      if (startIdx === -1) continue;
      
      // Check if the string ends on this line
      let searchFrom = startIdx + 1;
      let escaped = false;
      let ends = false;
      while (searchFrom < line.length) {
        const ch = line[searchFrom];
        if (escaped) {
          escaped = false;
        } else if (ch === '\\') {
          escaped = true;
        } else if (ch === '"') {
          ends = true;
          break;
        }
        searchFrom++;
      }
      if (!ends) {
        inString = true;
        quoteChar = '"';
      }
    } else {
      // We were in a string — check if it ends on this line
      let searchFrom = 0;
      let escaped = false;
      let ends = false;
      while (searchFrom < line.length) {
        const ch = line[searchFrom];
        if (escaped) {
          escaped = false;
        } else if (ch === '\\') {
          escaped = true;
        } else if (ch === quoteChar) {
          ends = true;
          break;
        }
        searchFrom++;
      }
      if (ends) {
        inString = false;
      } else {
        // Still in a string = multiline string literal
        return true;
      }
    }
  }
  
  return inString;
}

export function validateSplashBody(body: string): string | null {
  // Special marker for built-in chat panel — skip DSL validation
  if (body.trim() === "__chat__") {
    return null;
  }

  if (body.includes("if (")) {
    return "it used parenthesized `if` conditions; use `if cond { ... }` syntax instead";
  }

  // Check for multiline string literals
  if (hasMultilineStringLiteral(body)) {
    return "it contains a string literal with embedded newlines — Splash DSL strings cannot span multiple lines; use separate Label widgets per line instead";
  }

  const lines = body.split("\n");

  let braceDepth = 0;
  for (const line of lines) {
    const trimmed = line.trim();
    const normalized = trimmed.replace(/;+$/, "").trim();

    if (
      braceDepth === 0 &&
      normalized.length > 0 &&
      !normalized.startsWith("//") &&
      !normalized.startsWith("let ") &&
      !normalized.startsWith("fn ") &&
      !normalized.startsWith("if ") &&
      !normalized.startsWith("else")
    ) {
      const isTopLevelUiCall =
        normalized.startsWith("ui.") &&
        normalized.includes("(") &&
        normalized.endsWith(")");
      const open = normalized.indexOf("(");
      const isTopLevelHelperCall =
        open !== -1 &&
        /^[A-Za-z0-9_]+$/.test(normalized.slice(0, open).trim()) &&
        normalized.endsWith(")");

      if (isTopLevelUiCall || isTopLevelHelperCall) {
        return "it tried to run top-level initialization code like `sync_rows()`; the root container must be the final top-level expression, and initial widget values must be seeded directly in the declared UI";
      }
    }

    for (const ch of line) {
      if (ch === "{") {
        braceDepth += 1;
      } else if (ch === "}") {
        braceDepth = Math.max(0, braceDepth - 1);
      }
    }
  }

  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith("//")) {
      continue;
    }

    if (trimmed.startsWith("on_render:")) {
      return "it used `on_render`, which currently destabilizes embedded Makepad mini apps; declare fixed named widgets and update them directly instead";
    }
  }

  let insideTextInput = false;
  let textInputBraceDepth = 0;
  let textInputHasHeight = false;

  for (const line of lines) {
    const trimmed = line.trim();

    if (!insideTextInput) {
      const start = Math.max(trimmed.indexOf("TextInput{"), trimmed.indexOf("TextInput {"));
      if (start === -1) {
        continue;
      }

      insideTextInput = true;
      textInputBraceDepth = 0;
      textInputHasHeight = false;

      const snippet = trimmed.slice(start);
      if (!trimmed.startsWith("//") && snippet.includes("height:")) {
        textInputHasHeight = true;
        if (snippet.includes("height: Fit")) {
          return "it used `TextInput` with `height: Fit`; embedded Makepad text inputs must use a fixed numeric height such as `34`";
        }
      }

      for (const ch of snippet) {
        if (ch === "{") {
          textInputBraceDepth += 1;
        } else if (ch === "}") {
          textInputBraceDepth = Math.max(0, textInputBraceDepth - 1);
        }
      }

      if (textInputBraceDepth === 0) {
        if (!textInputHasHeight) {
          return "it declared `TextInput` without an explicit fixed height; use a numeric height such as `34` in embedded Makepad apps";
        }
        insideTextInput = false;
      }

      continue;
    }

    if (!trimmed.startsWith("//") && trimmed.includes("height:")) {
      textInputHasHeight = true;
      if (trimmed.includes("height: Fit")) {
        return "it used `TextInput` with `height: Fit`; embedded Makepad text inputs must use a fixed numeric height such as `34`";
      }
    }

    for (const ch of trimmed) {
      if (ch === "{") {
        textInputBraceDepth += 1;
      } else if (ch === "}") {
        textInputBraceDepth = Math.max(0, textInputBraceDepth - 1);
      }
    }

    if (textInputBraceDepth === 0) {
      if (!textInputHasHeight) {
        return "it declared `TextInput` without an explicit fixed height; use a numeric height such as `34` in embedded Makepad apps";
      }
      insideTextInput = false;
    }
  }

  const declaredIds = new Set<string>(["ui"]); // 'ui' is built-in in Splash DSL
  for (const line of lines) {
    const trimmed = line.trim();
    const idx = trimmed.indexOf(":=");
    if (idx === -1) {
      continue;
    }
    const before = trimmed.slice(0, idx).trim();
    const name = before.split(/\s+/).pop();
    if (name && /^[A-Za-z0-9_]+$/.test(name)) {
      declaredIds.add(name);
    }
  }

  for (const line of lines) {
    const err = looksLikeUnknownWidget(line);
    if (err) return err;
  }

  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith("//")) {
      continue;
    }

    const colon = trimmed.indexOf(":");
    if (colon === -1) {
      continue;
    }

    const beforeColon = trimmed.slice(0, colon);
    const token = (beforeColon.split(/\s+/).pop() ?? beforeColon)
      .split("{")
      .pop()
      ?.trim();

    if (!token || !token.includes(".")) {
      continue;
    }

    const root = token.split(".", 1)[0];
    if (PROPERTY_ROOTS.has(root)) {
      continue;
    }

    if (/^[A-Za-z0-9_]+$/.test(root) && !declaredIds.has(root)) {
      return `it referenced named child '${root}' without declaring '${root}' with ':=' first`;
    }
  }

  return null;
}
