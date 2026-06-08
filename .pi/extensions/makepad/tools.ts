import { Type } from "typebox";

import { connectToHarness, sendToHarness, onMessage } from "./doc-bridge.js";
import { startHarness, stopHarness } from "./harness.js";
import { STANDARD_APPS } from "./standard-apps.js";
import { validateSplashBody } from "./validate-splash.js";
import type { HarnessMessage, AppState } from "./types.js";

type ExtensionAPI = any;

// ── Lazy initialization ──────────────────────────────────────────────────

let harnessStarted = false;
let harnessReady: Promise<void> | null = null;
let currentApp: AppState | null = null;

async function ensureConnected(): Promise<void> {
  // If we think we're connected but the harness is dead, reset
  if (harnessStarted) {
    // Check if harness is actually alive by doing a quick connect test
    const { quickConnectCheck } = await import("./doc-bridge.js");
    const alive = await quickConnectCheck();
    if (!alive) {
      harnessStarted = false;
      harnessReady = null;
    } else {
      return;
    }
  }

  if (!harnessReady) {
    harnessReady = (async () => {
      try {
        startHarness(process.cwd());
        await connectToHarness();
        harnessStarted = true;
      } catch (err) {
        // Reset so subsequent calls can retry
        harnessReady = null;
        throw err;
      }
    })();
  }

  await harnessReady;
}

// ── Tool registration ───────────────────────────────────────────────────

export function registerTools(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "launch_makepad_app",
    label: "Launch Makepad App",
    description: "Launch a Makepad Splash mini-app in the host window.",
    promptSnippet:
      "Launch or replace a Makepad mini-app with generated Splash DSL",
    promptGuidelines: [
      "Use launch_makepad_app when the user asks to create, show, or update a native UI app.",
      "Generate only the Splash body - no Root{}, no Window{}, no Rust.",
      "Every TextInput must have a fixed numeric height such as 34.",
      "Do not use on_render in embedded Splash apps.",
    ],
    parameters: Type.Object({
      app_id: Type.String({
        description: "Unique ID for this app instance, e.g. 'counter-1'",
      }),
      splash_body: Type.String({
        description: "Makepad Splash body string (no Root/Window wrapping)",
      }),
      standard_app: Type.Optional(
        Type.String({
          description: "Optional standard app key, such as 'todo'",
        }),
      ),
    }),
    async execute(
      _id: string,
      params: any,
      _signal: AbortSignal,
      onUpdate: any,
    ) {
      try {
        await ensureConnected();
      } catch (err) {
        return {
          content: [
            { type: "text", text: `Failed to start Makepad harness: ${err}` },
          ],
          details: { error: String(err) },
          isError: true,
        };
      }

      const { app_id, standard_app } = params;
      const splash_body =
        typeof standard_app === "string" && STANDARD_APPS[standard_app]
          ? STANDARD_APPS[standard_app].splashBody
          : params.splash_body;

      const validationError = validateSplashBody(splash_body);
      if (validationError) {
        return {
          content: [
            {
              type: "text",
              text: `Splash validation failed: ${validationError}. Please fix and retry.`,
            },
          ],
          details: { error: validationError },
          isError: true,
        };
      }

      // Track locally
      currentApp = { app_id, status: "Pending", splash_body };

      // Send launch request over JSON WS
      sendToHarness({ type: "launch", app_id, splash_body });

      // Wait for status confirmation or error
      const launchResult = await new Promise<{ ok: boolean; message: string }>((resolve) => {
        const timeout = setTimeout(() => resolve({ ok: false, message: `Timed out awaiting confirmation for '${app_id}'.` }), 10_000);
        const unsub = onMessage((msg: HarnessMessage) => {
          // Check error FIRST — errors take priority over status
          if (msg.type === "error" && msg.app_id === app_id) {
            clearTimeout(timeout);
            unsub();
            if (currentApp) {
              currentApp.status = "Error";
            }
            resolve({ ok: false, message: `App '${app_id}' error: ${msg.message}` });
            return;
          }
          if (msg.type === "status" && msg.app_id === app_id) {
            if (currentApp) {
              currentApp.status = msg.status as AppState["status"];
            }
            clearTimeout(timeout);
            unsub();
            resolve({ ok: true, message: `App '${app_id}' launched.` });
          }
        });
      });

      return {
        content: [
          {
            type: "text",
            text: launchResult.message,
          },
        ],
        details: { app_id, launched: launchResult.ok },
        isError: !launchResult.ok,
      };
    },
  });

  pi.registerTool({
    name: "close_makepad_app",
    label: "Close Makepad App",
    description: "Close the currently running Makepad mini-app.",
    parameters: Type.Object({
      app_id: Type.String(),
    }),
    async execute(_id: string, params: any, _signal: AbortSignal) {
      try {
        await ensureConnected();
      } catch (err) {
        return {
          content: [
            { type: "text", text: `Failed to connect: ${err}` },
          ],
          details: { error: String(err) },
          isError: true,
        };
      }

      sendToHarness({ type: "clear", app_id: params.app_id });
      currentApp = null;

      return {
        content: [
          { type: "text", text: `App '${params.app_id}' closed.` },
        ],
        details: {},
      };
    },
  });

  pi.registerTool({
    name: "list_makepad_apps",
    label: "List Makepad Apps",
    description: "List the currently running Makepad mini-app.",
    parameters: Type.Object({}),
    async execute() {
      if (!currentApp) {
        return {
          content: [{ type: "text", text: "No app running." }],
          details: { apps: [] },
        };
      }

      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(
              {
                id: currentApp.app_id,
                status: currentApp.status,
                splash_preview: currentApp.splash_body.slice(0, 200),
              },
              null,
              2,
            ),
          },
        ],
        details: { apps: [currentApp] },
      };
    },
  });

  pi.registerTool({
    name: "store_value",
    label: "Store Value",
    description: "Store a persistent key-value pair accessible to mini-apps.",
    parameters: Type.Object({
      key: Type.String(),
      value: Type.String(),
      description: Type.String(),
    }),
    async execute() {
      // For now, just acknowledge — storage via CRDT is no longer used.
      return {
        content: [{ type: "text", text: "store_value noted (persistent storage not implemented yet)." }],
        details: {},
      };
    },
  });

  pi.registerTool({
    name: "read_value",
    label: "Read Value",
    description: "Read a stored key-value pair.",
    parameters: Type.Object({ key: Type.String() }),
    async execute() {
      return {
        content: [{ type: "text", text: "read_value not yet implemented." }],
        details: { found: false },
      };
    },
  });
}
