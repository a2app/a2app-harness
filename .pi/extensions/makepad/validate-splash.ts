export function validateSplashBody(body: string): string | null {
  // Special marker for built-in chat panel — skip DSL validation
  if (body.trim() === "__chat__") {
    return null;
  }

  if (body.includes("if (")) {
    return "it used parenthesized `if` conditions; use `if cond { ... }` syntax instead";
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

  const declaredIds = new Set<string>();
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

  const propertyRoots = new Set([
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
  ]);

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
    if (propertyRoots.has(root)) {
      continue;
    }

    if (/^[A-Za-z0-9_]+$/.test(root) && !declaredIds.has(root)) {
      return `it referenced named child '${root}' without declaring '${root}' with ':=' first`;
    }
  }

  return null;
}
