import { Type } from "typebox";
import { RawString } from "@automerge/automerge-repo";

import { connectToHarness, getDocHandle, waitForResponse } from "./doc-bridge";
import { startHarness } from "./harness";
import { watchInferenceRequests } from "./inference-loop";
import { STANDARD_APPS } from "./standard-apps";
import { validateSplashBody } from "./validate-splash";

type ExtensionAPI = any;

// Lazy initialization state: starts harness + connects on first tool call.
let harnessStarted = false;
let harnessReady: Promise<void> | null = null;
let inferencePoller: ReturnType<typeof setInterval> | null = null;
let _pi: ExtensionAPI | null = null;

/**
 * Ensures the harness + makepad-host are running, and we have a doc connection.
 * Called at the start of every tool execution.
 */
async function ensureConnected(): Promise<void> {
  if (harnessStarted) return;

  // Only attempt startup once; subsequent calls await the same promise.
  if (!harnessReady) {
    harnessReady = (async () => {
      startHarness(process.cwd());
      await connectToHarness();
      harnessStarted = true;

      // Start the inference polling loop (checks for Inference requests from mini-apps).
      if (_pi) {
        inferencePoller = watchInferenceRequests(_pi);
      }
    })();
  }

  await harnessReady;
}

/**
 * Clean up the inference poller. Called on session shutdown.
 */
export function stopInferencePoller(): void {
  if (inferencePoller) {
    clearInterval(inferencePoller);
    inferencePoller = null;
  }
}

export function registerTools(pi: ExtensionAPI): void {
  _pi = pi;
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
        Type.String({ description: "Optional standard app key, such as 'todo'" }),
      ),
    }),
    async execute(_id: string, params: any, _signal: AbortSignal, onUpdate: any) {
      // Lazy-init: start harness + connect on first use.
      try {
        await ensureConnected();
      } catch (err) {
        return {
          content: [{ type: "text", text: `Failed to start Makepad harness: ${err}` }],
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

      const docHandle = getDocHandle();
      docHandle.change((doc) => {
        doc.requests.push({
          LaunchApp: {
            id: new RawString(app_id) as unknown as string,
            splash_body: new RawString(splash_body) as unknown as string,
          },
        });
      });

      onUpdate?.({
        content: [{ type: "text", text: `Launching app '${app_id}'...` }],
      });

      // Use own timeout — the signal from the agent may already be aborted.
      const result = await waitForResponse(
        docHandle,
        (r): r is { AppLaunched: { id: string } } =>
          "AppLaunched" in r &&
          String((r as { AppLaunched: { id: unknown } }).AppLaunched?.id ?? "") === app_id,
        undefined,  // don't pass the agent signal — use our own timeout
      );

      const success = result !== null;

      return {
        content: [
          {
            type: "text",
            text: success
              ? `App '${app_id}' launched.`
              : `Launch timed out for '${app_id}'.`,
          },
        ],
        details: { app_id },
      };
    },
  });

  pi.registerTool({
    name: "close_makepad_app",
    label: "Close Makepad App",
    description: "Close a running Makepad mini-app by ID.",
    parameters: Type.Object({
      app_id: Type.String(),
    }),
    async execute(_id: string, params: any, _signal: AbortSignal) {
      try {
        await ensureConnected();
      } catch (err) {
        return {
          content: [{ type: "text", text: `Failed to start Makepad harness: ${err}` }],
          details: { error: String(err) },
          isError: true,
        };
      }

      const docHandle = getDocHandle();
      docHandle.change((doc) => {
        doc.requests.push({
          CloseApp: { id: new RawString(params.app_id) as unknown as string },
        });
      });

      const result = await waitForResponse(
        docHandle,
        (r): r is { AppClosed: { id: string } } =>
          "AppClosed" in r &&
          String((r as { AppClosed: { id: unknown } }).AppClosed?.id ?? "") === params.app_id,
        undefined,
      );

      return {
        content: [
          {
            type: "text",
            text: result ? `App '${params.app_id}' closed.` : "Close timed out.",
          },
        ],
        details: {},
      };
    },
  });

  pi.registerTool({
    name: "list_makepad_apps",
    label: "List Makepad Apps",
    description: "List currently running Makepad mini-apps and their Splash bodies.",
    parameters: Type.Object({}),
    async execute() {
      try {
        await ensureConnected();
      } catch (err) {
        return {
          content: [{ type: "text", text: `Failed to start Makepad harness: ${err}` }],
          details: { error: String(err) },
          isError: true,
        };
      }

      const doc = getDocHandle().doc();
      if (!doc) {
        return {
          content: [{ type: "text", text: "Doc not ready." }],
          details: {},
        };
      }

      const apps = Object.entries(doc.mini_apps).map(([id, app]) => ({
        id,
        splash_preview: String(app.splash_body).slice(0, 200),
      }));

      return {
        content: [{ type: "text", text: JSON.stringify(apps, null, 2) }],
        details: { apps },
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
    async execute(_id: string, params: any) {
      try {
        await ensureConnected();
      } catch (err) {
        return {
          content: [{ type: "text", text: `Failed to start Makepad harness: ${err}` }],
          details: { error: String(err) },
          isError: true,
        };
      }

      getDocHandle().change((doc) => {
        doc.stored_values[params.key] = {
          value: params.value,
          description: params.description,
        };
      });
      return {
        content: [{ type: "text", text: `Stored '${params.key}'.` }],
        details: {},
      };
    },
  });

  pi.registerTool({
    name: "read_value",
    label: "Read Value",
    description: "Read a stored key-value pair.",
    parameters: Type.Object({ key: Type.String() }),
    async execute(_id: string, params: any) {
      try {
        await ensureConnected();
      } catch (err) {
        return {
          content: [{ type: "text", text: `Failed to start Makepad harness: ${err}` }],
          details: { error: String(err) },
          isError: true,
        };
      }

      const doc = getDocHandle().doc();
      const entry = doc?.stored_values[params.key];
      return {
        content: [
          {
            type: "text",
            text: entry ? entry.value : `Key '${params.key}' not found.`,
          },
        ],
        details: { found: Boolean(entry), value: entry?.value },
      };
    },
  });
}
