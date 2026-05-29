import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { getDocHandle } from "./doc-bridge";
import { stopHarness } from "./harness";
import { registerTools, stopInferencePoller } from "./tools";

let extensionDir = "";

// Load prompt guidance from the markdown file (single source of truth).
function loadMakepadPrompt(extensionDir: string): string {
  try {
    const promptPath = resolve(extensionDir, "prompts", "makepad-environment.md");
    return readFileSync(promptPath, "utf-8");
  } catch {
    // Fallback if file is missing at runtime.
    return `You are operating in a Makepad mini-app environment.

Constraints:
- Generate Splash body only. Do not emit Root{}, Window{}, or Rust wrappers.
- Avoid on_render in embedded apps.
- Every TextInput must use a fixed numeric height like 34.
- Keep layouts simple and deterministic.

Tools:
- Use launch_makepad_app to create or update native Splash mini apps.
- Use close_makepad_app to remove or close an app.
- Use list_makepad_apps to list running apps.
- Use store_value / read_value for persistent data.`;
  }
}

export default async function (pi: ExtensionAPI): Promise<void> {
  registerTools(pi);

  pi.on("session_start", async (_event: any, ctx: any) => {
    extensionDir = ctx.extensionPath ?? __dirname ?? "";
    // Do NOT start the harness on session start.
    // It will be started lazily on the first tool call via tools.ts.
    ctx.ui.setStatus("makepad", "Makepad: idle (start on first use)");
  });

  pi.on("session_shutdown", async () => {
    // Stop the inference polling loop.
    stopInferencePoller();

    // Signal graceful shutdown to makepad-host via the shared doc.
    try {
      getDocHandle().change((doc) => {
        doc.should_exit = true;
      });
    } catch {
      // No connected doc.
    }

    stopHarness();
  });

  pi.on("before_agent_start", async (event: any) => {
    try {
      const doc = getDocHandle().doc();
      if (!doc) {
        // Harness not yet started — no Makepad context to add.
        return;
      }

      const promptBase = loadMakepadPrompt(extensionDir);

      const appIds = Object.keys(doc.mini_apps);
      const runningAppsLine =
        appIds.length > 0
          ? `\n\nCurrently running Makepad apps: ${appIds.join(", ")}.`
          : "\n\nCurrently running Makepad apps: none.";

      return { systemPrompt: `${event.systemPrompt}\n\n${promptBase}${runningAppsLine}` };
    } catch {
      return;
    }
  });

  // Track file reads/writes in the shared doc (no harness lazy-init here).
  pi.on("tool_call", async (event: any) => {
    try {
      const docHandle = getDocHandle();

      const input = event.input ?? event.args ?? {};

      if (event.toolName === "write" || event.toolName === "edit") {
        const path = String(input.filePath ?? input.path ?? "");
        const content = String(
          input.content ?? input.newContent ?? "",
        );
        if (path.length > 0) {
          docHandle.change((doc) => {
            doc.text_documents[path] = content;
            doc.active_document = path;
          });
        }
      }

      if (event.toolName === "read") {
        const path = String(input.filePath ?? input.path ?? "");
        if (path.length > 0) {
          docHandle.change((doc) => {
            doc.active_document = path;
          });
        }
      }
    } catch {
      // Ignore tool tracking errors when doc is disconnected.
    }
  });
}
