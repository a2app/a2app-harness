import { readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { stopHarness } from "./harness.js";
import { registerTools } from "./tools.js";
import { registerBackgroundAgentTools, disposeAllSessions, startAutoBackgroundHandler } from "./background-agent.js";
import { connectToHarness, onMessage } from "./doc-bridge.js";
import type { HarnessMessage } from "./types.js";

const _dirname = typeof __dirname !== "undefined" ? __dirname : dirname(fileURLToPath(import.meta.url));
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
  registerBackgroundAgentTools(pi);

  // Start the auto-handler for ai:* messages from the splash app
  // This enables the ai-chat standard app to work without pi agent involvement
  startAutoBackgroundHandler();

  pi.on("session_start", async (_event: any, ctx: any) => {
    extensionDir = ctx.extensionPath ?? _dirname ?? "";
    ctx.ui.setStatus("makepad", "Makepad: idle (start on first use)");
  });

  pi.on("session_shutdown", async () => {
    try {
      const { sendToHarness } = await import("./doc-bridge.js");
      sendToHarness({ type: "exit" });
    } catch {
      // Not connected.
    }

    disposeAllSessions();
    stopHarness();
  });

  pi.on("before_agent_start", async (event: any) => {
    // Try to connect and get app info for the system prompt.
    // We don't block the agent on this — it's best-effort.
    const runningAppLine = await getRunningAppInfo();

    const promptBase = loadMakepadPrompt(extensionDir);
    return {
      systemPrompt: `${event.systemPrompt}\n\n${promptBase}${runningAppLine}`,
    };
  });
}

async function getRunningAppInfo(): Promise<string> {
  try {
    const { quickConnectCheck } = await import("./doc-bridge.js");
    const alive = await quickConnectCheck();
    if (alive) {
      return "\n\nCurrently running Makepad apps: see list_makepad_apps tool.";
    }
  } catch {
    // ignore
  }
  return "\n\nMakepad harness not running. Apps will launch on first use.";
}
