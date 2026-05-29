import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { connectToHarness, getDocHandle } from "./doc-bridge";
import { startHarness, stopHarness } from "./harness";
import { watchInferenceRequests } from "./inference-loop";
import { registerTools } from "./tools";

const MAKEPAD_PROMPT_APPEND = `
Makepad runtime guidance:
- Use launch_makepad_app to create or update native Splash mini apps.
- Generate Splash body only; do not include Root{}, Window{}, or Rust wrappers.
- Avoid on_render in embedded mini apps.
- Give every TextInput an explicit numeric height (for example 34).
- Use list_makepad_apps before replacing an unknown app.
- Use close_makepad_app when asked to remove an app.
- Use store_value/read_value for persistent app data.
`;

export default async function (pi: ExtensionAPI): Promise<void> {
  let inferencePoller: ReturnType<typeof setInterval> | null = null;

  registerTools(pi);

  pi.on("session_start", async (_event: any, ctx: any) => {
    ctx.ui.setStatus("makepad", "Makepad: starting...");
    try {
      startHarness(ctx.cwd);
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
      const doc = getDocHandle().doc();
      if (!doc) {
        return;
      }

      const appIds = Object.keys(doc.mini_apps);
      const runningAppsLine =
        appIds.length > 0
          ? `\n\nCurrently running Makepad apps: ${appIds.join(", ")}.`
          : "\n\nCurrently running Makepad apps: none.";

      return { systemPrompt: event.systemPrompt + MAKEPAD_PROMPT_APPEND + runningAppsLine };
    } catch {
      return;
    }
  });

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
