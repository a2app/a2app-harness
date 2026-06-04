import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { stopHarness } from "./harness.js";
import { registerTools } from "./tools.js";

let extensionDir = "";

function loadMakepadPrompt(extensionDir: string): string {
  try {
    const promptPath = resolve(
      extensionDir,
      "prompts",
      "makepad-environment.md",
    );
    return readFileSync(promptPath, "utf-8");
  } catch {
    return `You are operating in a Makepad mini-app environment.

Constraints:
- Generate Splash body only. Do not emit Root{}, Window{}, or Rust wrappers.
- Avoid on_render in embedded apps.
- Every TextInput must use a fixed numeric height like 34.
- Keep layouts simple and deterministic.

Tools:
- Use launch_makepad_app to create or update a native Splash mini app.
- Use close_makepad_app to remove the current app.
- Use list_makepad_apps to list the running app.`;
  }
}

export default async function (pi: ExtensionAPI): Promise<void> {
  registerTools(pi);

  pi.on("session_start", async (_event: any, ctx: any) => {
    extensionDir = ctx.extensionPath ?? __dirname ?? "";
    ctx.ui.setStatus("makepad", "Makepad: idle (start on first use)");
  });

  pi.on("session_shutdown", async () => {
    // Signal graceful shutdown via the shared doc.
    try {
      const { getDocHandle } = await import("./doc-bridge.js");
      getDocHandle().change((doc: any) => {
        doc.should_exit = true;
      });
    } catch {
      // Not connected.
    }

    stopHarness();
  });

  pi.on("before_agent_start", async (event: any) => {
    try {
      const { getDocHandle } = await import("./doc-bridge.js");
      const doc = getDocHandle().doc();
      const promptBase = loadMakepadPrompt(extensionDir);

      const runningAppLine = doc?.pending_app
        ? `\n\nCurrently running Makepad app: ${doc.pending_app.id} (${doc.pending_app.status}).`
        : "\n\nCurrently running Makepad apps: none.";

      return {
        systemPrompt: `${event.systemPrompt}\n\n${promptBase}${runningAppLine}`,
      };
    } catch {
      return;
    }
  });
}
