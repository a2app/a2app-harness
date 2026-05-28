import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { connectToHarness, getDocHandle } from "./doc-bridge.js";
import { startHarness, stopHarness } from "./harness.js";
import { watchInferenceRequests } from "./inference-loop.js";
import { registerTools } from "./tools.js";

const EXTENSION_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export default async function (pi: ExtensionAPI): Promise<void> {
  let inferencePoller: ReturnType<typeof setInterval> | null = null;

  registerTools(pi);

  pi.on("session_start", async (_event: any, ctx: any) => {
    ctx.ui.setStatus("makepad", "Makepad: starting...");
    try {
      startHarness(EXTENSION_ROOT);
      await connectToHarness();
      inferencePoller = watchInferenceRequests(pi);
      ctx.ui.setStatus("makepad", "Makepad: ready");
      ctx.ui.notify("Makepad host connected", "info");
    } catch (err) {
      ctx.ui.setStatus("makepad", "Makepad: offline");
      ctx.ui.notify(`Makepad host failed to start: ${String(err)}`, "error");
    }
  });

  pi.on("session_shutdown", async () => {
    if (inferencePoller) {
      clearInterval(inferencePoller);
      inferencePoller = null;
    }

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
      const doc = getDocHandle().docSync();
      if (!doc) {
        return;
      }

      const appIds = Object.keys(doc.mini_apps);
      if (appIds.length === 0) {
        return;
      }

      return {
        systemPrompt:
          event.systemPrompt +
          `\n\nCurrently running Makepad apps: ${appIds.join(", ")}.`,
      };
    } catch {
      return;
    }
  });

  pi.on("tool_call", async (event: any) => {
    try {
      const docHandle = getDocHandle();
      if (event.toolName === "write" || event.toolName === "edit") {
        const path = String(event.args?.filePath ?? event.args?.path ?? "");
        const content = String(
          event.args?.content ?? event.args?.newContent ?? "",
        );
        if (path.length > 0) {
          docHandle.change((doc) => {
            doc.text_documents[path] = content;
            doc.active_document = path;
          });
        }
      }

      if (event.toolName === "read") {
        const path = String(event.args?.filePath ?? event.args?.path ?? "");
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
