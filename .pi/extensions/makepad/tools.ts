import { Type } from "typebox";
import { RawString } from "@automerge/automerge-repo";

import { getDocHandle, waitForResponse } from "./doc-bridge";
import { STANDARD_APPS } from "./standard-apps";
import { validateSplashBody } from "./validate-splash";

type ExtensionAPI = any;

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
        Type.String({ description: "Optional standard app key, such as 'todo'" }),
      ),
    }),
    async execute(_id: string, params: any, signal: AbortSignal, onUpdate: any) {
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

      const result = await waitForResponse(
        docHandle,
        (r): r is { AppLaunched: { id: string } } =>
          "AppLaunched" in r &&
          String((r as { AppLaunched: { id: unknown } }).AppLaunched?.id ?? "") === app_id,
        signal,
      );

      return {
        content: [
          {
            type: "text",
            text: result
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
    async execute(_id: string, params: any, signal: AbortSignal) {
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
        signal,
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
      const doc = getDocHandle().doc();
      if (!doc) {
        return {
          content: [{ type: "text", text: "Doc not ready." }],
          details: {},
        };
      }

      const apps = Object.entries(doc.mini_apps).map(([id, app]) => ({
        id,
        splash_preview: app.splash_body.slice(0, 200),
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
