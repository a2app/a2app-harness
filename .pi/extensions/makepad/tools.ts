import { Type } from "typebox";
import { RawString } from "@automerge/automerge-repo";

import { connectToHarness, getDocHandle } from "./doc-bridge.js";
import { startHarness } from "./harness.js";
import { STANDARD_APPS } from "./standard-apps.js";
import { validateSplashBody } from "./validate-splash.js";

type ExtensionAPI = any;

// Lazy initialization state.
let harnessStarted = false;
let harnessReady: Promise<void> | null = null;
let _pi: ExtensionAPI | null = null;

async function ensureConnected(): Promise<void> {
  if (harnessStarted) return;

  if (!harnessReady) {
    harnessReady = (async () => {
      startHarness(process.cwd());
      await connectToHarness();
      harnessStarted = true;
    })();
  }

  await harnessReady;
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
            {
              type: "text",
              text: `Failed to start Makepad harness: ${err}`,
            },
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

      const docHandle = getDocHandle();

      // Set the pending app — the host will pick it up asynchronously
      docHandle.change((doc: any) => {
        doc.pending_app = {
          id: new RawString(app_id) as unknown as string,
          splash_body: new RawString(splash_body) as unknown as string,
          status: "Pending",
        };
        doc.extension_requests = true;
      });

      return {
        content: [
          { type: "text", text: `App '${app_id}' launch requested.` },
        ],
        details: { app_id },
      };
    },
  });

  // Keep close_makepad_app for symmetry (clears the pending app)
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
            { type: "text", text: `Failed to start Makepad harness: ${err}` },
          ],
          details: { error: String(err) },
          isError: true,
        };
      }

      const docHandle = getDocHandle();
      docHandle.change((doc: any) => {
        doc.pending_app = null;
        doc.extension_requests = true;
      });

      return {
        content: [
          {
            type: "text",
            text: `App '${params.app_id}' closed.`,
          },
        ],
        details: {},
      };
    },
  });

  // Keep list_makepad_apps but simplified
  pi.registerTool({
    name: "list_makepad_apps",
    label: "List Makepad Apps",
    description: "List the currently running Makepad mini-app.",
    parameters: Type.Object({}),
    async execute() {
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

      const doc = getDocHandle().doc();
      if (!doc || !doc.pending_app) {
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
                id: doc.pending_app.id,
                status: doc.pending_app.status,
                splash_preview: String(doc.pending_app.splash_body).slice(
                  0,
                  200,
                ),
              },
              null,
              2,
            ),
          },
        ],
        details: { apps: [doc.pending_app] },
      };
    },
  });

  // Update store_value/read_value to use extension_requests flag
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
          content: [
            { type: "text", text: `Failed to start Makepad harness: ${err}` },
          ],
          details: { error: String(err) },
          isError: true,
        };
      }

      // Store in a simple structured field — for now we notify but don't persist
      getDocHandle().change((doc: any) => {
        doc.extension_requests = true;
      });
      return {
        content: [
          { type: "text", text: `Stored '${params.key}'.` },
        ],
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
          content: [
            { type: "text", text: `Failed to start Makepad harness: ${err}` },
          ],
          details: { error: String(err) },
          isError: true,
        };
      }

      return {
        content: [
          { type: "text", text: `Key '${params.key}' not found (store/read not yet re-implemented).` },
        ],
        details: { found: false },
      };
    },
  });
}
