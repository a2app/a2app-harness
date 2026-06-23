import { Type } from "typebox";

import { connectToHarness, sendToHarness, onMessage, getBufferedEvent, getAllBufferedEvents, clearEventBuffer } from "./doc-bridge.js";
import { startHarness, stopHarness } from "./harness.js";
import { validateSplashBody } from "./validate-splash.js";
import { registerAppSessionAssociation } from "./background-agent.js";
import type { HarnessMessage, AppState, GetDocMessage } from "./types.js";

type ExtensionAPI = any;

// ── Lazy initialization ──────────────────────────────────────────────────

let harnessStarted = false;
let harnessReady: Promise<void> | null = null;
let currentApp: AppState | null = null;

// Track the last render error per app_id
const lastErrors = new Map<string, string>();

// Track pending error listeners (for the debounce window after launch)
let pendingErrorListener: (() => void) | null = null;

export async function ensureConnected(): Promise<void> {
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
      "Launch or replace a Makepad mini-app with Splash DSL",
    promptGuidelines: [
      "Use launch_makepad_app when the user asks to create, show, or update a native UI app.",
      "Generate only the Splash body - no Root{}, no Window{}, no Rust.",
      "Every TextInput must have a fixed numeric height such as 34.",
      "Do not use on_render in embedded Splash apps.",
      "AFTER launching, use check_debug_app with debug_command=widget_snapshot to verify the app rendered correctly and discover widget coordinates.",
      "Container must always have height:Fit.",
      "To send data from splash to pi, use ui.__pi_response.set_text('...') inside on_click handlers.",
      "Always check list_makepad_apps or check_debug_app for errors if app doesn't render.",
    ],
    parameters: Type.Object({
      app_id: Type.String({
        description: "Unique ID for this app instance, e.g. 'counter-1'",
      }),
      splash_body: Type.String({
        description: "Makepad Splash body string (no Root/Window wrapping)",
      }),
      agent_session_id: Type.Optional(
        Type.String({
          description: "Optional session ID from start_background_session. Associates this app with a background agent.",
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

      const { app_id } = params;
      const splash_body = params.splash_body;

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

      // Clear any previous error for this app_id
      lastErrors.delete(app_id);

      // If agent_session_id is provided, associate it with this app
      if (params.agent_session_id) {
        registerAppSessionAssociation(app_id, params.agent_session_id);
      }

      // Send launch request over JSON WS
      sendToHarness({ type: "launch", app_id, splash_body });

      // Wait for status confirmation or error with a debounce window.
      // Status arrives before error (harness writes status immediately, but
      // rendering happens asynchronously on makepad-host via CRDT sync).
      // We wait for status, then hold a 1.5s debounce window for errors.
      const launchResult = await new Promise<{ ok: boolean; message: string }>((resolve) => {
        const timeout = setTimeout(() => resolve({ ok: false, message: `Timed out awaiting confirmation for '${app_id}'.` }), 12_000);
        let statusReceived = false;
        let statusTimer: ReturnType<typeof setTimeout> | null = null;
        let listenerActive = true;

        const unsub = onMessage((msg: HarnessMessage) => {
          if (!listenerActive) return;

          if (msg.type === "error" && msg.app_id === app_id) {
            clearTimeout(timeout);
            if (statusTimer) clearTimeout(statusTimer);
            listenerActive = false;
            unsub();
            lastErrors.set(app_id, msg.message);
            if (currentApp) {
              currentApp.status = "Error";
              currentApp.last_error = msg.message;
            }
            resolve({ ok: false, message: `App '${app_id}' render error: ${msg.message}.` });
            return;
          }

          // Only start debounce on "Launched" status, not "Pending".
          // The harness now writes Launched immediately after Pending.
          // This ensures errors from makepad-host arrive within the debounce window.
          if (msg.type === "status" && msg.status === "Launched" && msg.app_id === app_id && !statusReceived) {
            statusReceived = true;
            if (currentApp) {
              currentApp.status = msg.status as AppState["status"];
            }
            // Debounce: wait 1.5s after Launched status before resolving,
            // to catch any rendering error that follows
            statusTimer = setTimeout(() => {
              clearTimeout(timeout);
              listenerActive = false;
              unsub();
              resolve({ ok: true, message: `App '${app_id}' launched.` });
            }, 1500);
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
      lastErrors.delete(params.app_id);

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
    description: "List the currently running Makepad mini-app and any last render error.",
    parameters: Type.Object({}),
    async execute() {
      if (!currentApp) {
        return {
          content: [{ type: "text", text: "No app running." }],
          details: { apps: [] },
        };
      }

      const errorInfo = currentApp.last_error
        ? currentApp.last_error
        : lastErrors.get(currentApp.app_id);

      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(
              {
                id: currentApp.app_id,
                status: currentApp.status,
                error: errorInfo || null,
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
    name: "check_debug_app",
    label: "Check/Debug App",
    description:
      "Inspect the running app's widget tree, check errors, retry with corrected body, or simulate interactions (click, type text).",
    promptSnippet:
      "Inspect or interact with the running Makepad mini-app",
    promptGuidelines: [
      "Use check_debug_app to inspect the widget tree, query widgets, or simulate interactions.",
      "Available debug_command values:",
      "  - widget_snapshot: Returns full JSON array of all widgets with id, widget_type, x, y, width, height, text, value. Pass debug_params='{}'.",
      "  - widget_dump: Compact text tree. Pass debug_params='{}'.",
      "  - widget_query: Search by id or type. Pass query string as debug_params.",
      "  - click: Simulate mouse click. Calculate center as (x + w/2, y + h/2) from snapshot data. Pass JSON as debug_params, e.g. '{\"x\":100,\"y\":200}'.",
      "  - type_text: Fill the FIRST TextInput in the splash tree. Pass raw text as debug_params (not JSON). Does NOT trigger on_return callbacks.",
      "",
      "STANDARD WORKFLOW for interactivity:",
      "  1. launch_makepad_app (create app)",
      "  2. check_debug_app (widget_snapshot, {}) — discover widget positions",
      "  3. check_debug_app (type_text, 'text') — fill first TextInput (optional)",
      "  4. check_debug_app (click, '{\"x\":100,\"y\":200}') — click button (center = x+w/2, y+h/2)",
      "  5. check_debug_app (widget_snapshot, {}) — verify __pi_response label text changed",
      "  6. check_debug_app (click, ...) — next interaction",
      "",
      "IMPORTANT:",
      "  - Splash content widgets have parent=-1 (orphaned). Do NOT use widget_id for click — use x,y coordinates.",
      "  - widget_snapshot includes orphaned widgets at the bottom of the JSON array.",
      "  - __pi_response initial text is ' ' (space). After click, it shows the response string.",
      "  - The 'value' field on TextInput shows the text content; 'text' field is null for TextInputs.",
      "  - Splash VM CAN read values set by Rust's set_text() via ui.<name>.text().",
    ],
    parameters: Type.Object({
      app_id: Type.Optional(
        Type.String({
          description: "App ID to debug (defaults to current app)",
        }),
      ),
      retry_splash_body: Type.Optional(
        Type.String({
          description:
            "Optional corrected splash body to re-launch (replaces the current app)",
        }),
      ),
      debug_command: Type.Optional(
        Type.String({
          description:
            "Debug command: widget_dump, widget_snapshot, widget_query, click, type_text",
        }),
      ),
      debug_params: Type.Optional(
        Type.String({
          description:
            "Parameters for the debug command (JSON-encoded string). For click: JSON with widget_id or x,y. For type_text: raw text string.",
        }),
      ),
      timeout_seconds: Type.Optional(
        Type.Number({
          description:
            "Max seconds to wait for debug response (default 10, max 30)",
        }),
      ),
    }),
    async execute(
      _id: string,
      params: any,
      _signal: AbortSignal,
      onUpdate: any,
    ) {
      const appId = params.app_id || currentApp?.app_id;
      if (!appId) {
        return {
          content: [
            {
              type: "text",
              text: "No app specified and no current app is running. Provide an app_id or launch an app first.",
            },
          ],
          details: {},
        };
      }

      // ── Debug command mode ────────────────────────────────────────
      if (params.debug_command) {
        try {
          await ensureConnected();
        } catch (err) {
          return {
            content: [{ type: "text", text: `Failed to connect: ${err}` }],
            details: { error: String(err) },
            isError: true,
          };
        }

        const cmd = params.debug_command;
        const raw_params = params.debug_params || "{}";
        const timeout = Math.min(params.timeout_seconds || 10, 30) * 1000;

        sendToHarness({ type: "debug", app_id: appId, command: cmd, params: raw_params });

        const rawResult = await new Promise<string>((resolve, reject) => {
          const timer = setTimeout(() => {
            unsub();
            reject(new Error(`Timed out after ${timeout / 1000}s waiting for debug response.`));
          }, timeout);
          let active = true;
          const unsub = onMessage((msg: HarnessMessage) => {
            if (!active) return;
            if (msg.type === "debug_response" && msg.app_id === appId) {
              clearTimeout(timer);
              active = false;
              unsub();
              resolve(msg.result);
            }
          });
        });

        let formatted = rawResult;
        let isErr = false;
        try {
          const parsed = JSON.parse(rawResult);
          if (parsed.error) {
            isErr = true;
            formatted = `Error: ${parsed.error}`;
          } else if (parsed.status === "pending") {
            if (cmd === "click") {
              formatted = `Click queued at (${parsed.x}, ${parsed.y}).`;
            } else if (cmd === "type_text") {
              formatted = `Text "${parsed.text}" queued for input.`;
            }
          }
        } catch {
          // raw text output
        }

        return {
          content: [{ type: "text", text: formatted }],
          details: { app_id: appId, command: cmd, result: rawResult },
          isError: isErr,
        };
      }

      // ── Retry splash body mode ────────────────────────────────────
      if (params.retry_splash_body) {
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

        const splash_body = params.retry_splash_body;
        const validationError = validateSplashBody(splash_body);
        if (validationError) {
          return {
            content: [
              {
                type: "text",
                text: `Validation failed: ${validationError}. Please fix and retry.`,
              },
            ],
            details: { error: validationError },
            isError: true,
          };
        }

        currentApp = { app_id: appId, status: "Pending", splash_body };
        lastErrors.delete(appId);
        sendToHarness({ type: "launch", app_id: appId, splash_body });

        const launchResult = await new Promise<{ ok: boolean; message: string }>(
          (resolve) => {
            const timeout = setTimeout(
              () =>
                resolve({
                  ok: false,
                  message: `Timed out awaiting confirmation for '${appId}'.`,
                }),
              12_000,
            );
            let statusReceived = false;
            let statusTimer: ReturnType<typeof setTimeout> | null = null;
            let active = true;

            const unsub = onMessage((msg: HarnessMessage) => {
              if (!active) return;

              if (msg.type === "error" && msg.app_id === appId) {
                clearTimeout(timeout);
                if (statusTimer) clearTimeout(statusTimer);
                active = false;
                unsub();
                lastErrors.set(appId, msg.message);
                if (currentApp) {
                  currentApp.status = "Error";
                  currentApp.last_error = msg.message;
                }
                resolve({
                  ok: false,
                  message: `App '${appId}' render error: ${msg.message}.`,
                });
                return;
              }

              if (msg.type === "status" && msg.status === "Launched" && msg.app_id === appId && !statusReceived) {
                statusReceived = true;
                if (currentApp) {
                  currentApp.status = msg.status as AppState["status"];
                }
                statusTimer = setTimeout(() => {
                  clearTimeout(timeout);
                  active = false;
                  unsub();
                  resolve({
                    ok: true,
                    message: `App '${appId}' re-launched.`,
                  });
                }, 1500);
              }
            });
          },
        );

        return {
          content: [{ type: "text", text: launchResult.message }],
          details: { app_id: appId, launched: launchResult.ok },
          isError: !launchResult.ok,
        };
      }

      // ── Report mode (default) ────────────────────────────────────
      const storedError = lastErrors.get(appId);
      const currentAppForId = currentApp?.app_id === appId ? currentApp : null;
      const currentError = currentAppForId?.last_error;
      const error = currentError || storedError;

      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(
              {
                app_id: appId,
                running: currentAppForId !== null,
                status: currentAppForId?.status || "unknown",
                error: error || null,
                hint: currentAppForId
                  ? (error
                    ? "Use check_debug_app with retry_splash_body set to a corrected Splash body to re-launch."
                    : "No errors. Use check_debug_app debug_command='widget_snapshot' debug_params='{}' to inspect the app.")
                  : "No app running. Use launch_makepad_app to create one first.",
              },
              null,
              2,
            ),
          },
        ],
        details: { app_id: appId, error: error || null },
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

  // ── Doc Inspector Tool ───────────────────────────────────────────────
  //
  // Queries the harness for the current state of the shared CRDT document.
  // This is a read-only snapshot of all fields: pending_app, user_response,
  // error_message, status. Use this to check if a splash app has sent a
  // response, check for render errors, or see what app is currently running.
  //
  pi.registerTool({
    name: "inspect_makepad_doc",
    label: "Inspect Makepad Doc",
    description:
      "Query the current state of the shared CRDT document (pending_app, user_response, error_message, status).",
    promptSnippet:
      "Inspect the shared CRDT document state",
    promptGuidelines: [
      "Use inspect_makepad_doc to read the full shared document state from the harness.",
      "Returns JSON: { app_id, user_response, error_message, status }",
      "Also checks the local event buffer for any user_response that arrived between tool calls.",
      "Clears the buffer after reading.",
      "",
      "USE CASES:",
      "  - Check if a splash app has sent a response via __pi_response.set_text()",
      "  - Check for render errors (error_message field)",
      "  - See what app is currently running",
      "  - Use AFTER a click to confirm the response was captured in the doc",
    ],
    parameters: Type.Object({}),
    async execute(
      _id: string,
      _params: any,
      _signal: AbortSignal,
      _onUpdate: any,
    ) {
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

      // Send get_doc request
      sendToHarness({ type: "get_doc" });

      // Wait for doc_state response (with timeout)
      const result = await new Promise<{
        app_id: string | null;
        user_response: string | null;
        error_message: string | null;
        status: string | null;
      }>((resolve, reject) => {
        const timeout = setTimeout(() => {
          unsub();
          reject(new Error("Timed out waiting for doc_state response"));
        }, 5_000);
        let active = true;
        const unsub = onMessage((msg: HarnessMessage) => {
          if (!active) return;
          if (msg.type === "doc_state") {
            clearTimeout(timeout);
            active = false;
            unsub();
            resolve({
              app_id: msg.app_id,
              user_response: msg.user_response,
              error_message: msg.error_message,
              status: msg.status,
            });
          }
        });
      });

      // Also check the buffer for any user_response that arrived recently
      const bufferedResponse = getBufferedEvent("user_response");
      let bufferedText = "";
      if (bufferedResponse && bufferedResponse.type === "user_response") {
        bufferedText = ` [buffered: ${bufferedResponse.response}]`;
      }

      // Also check buffer for errors
      const bufferedError = getBufferedEvent("error");
      let bufferedErrorText = "";
      if (bufferedError && bufferedError.type === "error") {
        bufferedErrorText = ` [buffered: ${bufferedError.message}]`;
      }

      // Clear the buffer so we don't re-read stale events
      clearEventBuffer();

      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(
              {
                app_id: result.app_id,
                user_response: result.user_response,
                error_message: result.error_message || bufferedErrorText.replace(/^ \[buffered: /, "").replace(/\]$/, "") || null,
                status: result.status,
              },
              null,
              2,
            ),
          },
        ],
        details: result,
      };
    },
  });

  // ── Wait-for-Response Tool ───────────────────────────────────────────
  //
  // An event-driven listener that blocks until a user_response is received
  // from the splash app. This enables a "service worker" pattern:
  //
  //   1. Launch an app that sends responses (via __pi_response.set_text())
  //   2. Call wait_for_response — it sets up a persistent listener
  //   3. The tool awaits the response, with configurable timeout
  //   4. When the splash app calls __pi_response.set_text("..."), the
  //      response flows through: CRDT doc → harness bridge → JSON WS → tool
  //
  // The response is also buffered in doc-bridge.ts so other tools can
  // inspect it later via inspect_makepad_doc or by checking the buffer.
  //
  pi.registerTool({
    name: "wait_for_response",
    label: "Wait for Response",
    description:
      "Block and wait for a user_response from the splash app. Sets up an event-driven listener that triggers when the splash app calls __pi_response.set_text().",
    promptSnippet:
      "Wait for splash app response (event-driven listener)",
    promptGuidelines: [
      "Use wait_for_response to asynchronously receive data from the splash app.",
      "The splash app sends data by calling ui.__pi_response.set_text('...') in its on_click handler.",
      "This is the primary way for native UI apps to communicate back to the pi agent.",
      "",
      "STANDARD FLOW:",
      "  1. Launch app with ui.__pi_response.set_text() buttons",
      "  2. Call wait_for_response (app_id='my-app', timeout_seconds=30)",
      "  3. Tool blocks until user clicks a button OR timeout expires",
      "  4. Returns { app_id, response, source: 'live' | 'buffered' }",
      "",
      "Parameters: app_id (optional, defaults to current), timeout_seconds (default 30, max 120), clear_buffer (default true)",
      "",
      "If a response was already buffered (arrived between tool calls), returns immediately with source='buffered'.",
    ],
    parameters: Type.Object({
      app_id: Type.Optional(
        Type.String({
          description: "App ID to wait for a response from (defaults to current app)",
        }),
      ),
      timeout_seconds: Type.Optional(
        Type.Number({
          description: "Max seconds to wait (default 30, max 120)",
        }),
      ),
      clear_buffer: Type.Optional(
        Type.Boolean({
          description: "Clear any previous buffered response before waiting (default true)",
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
            { type: "text", text: `Failed to connect: ${err}` },
          ],
          details: { error: String(err) },
          isError: true,
        };
      }

      const appId = params.app_id || currentApp?.app_id;
      if (!appId && !params.app_id) {
        return {
          content: [
            {
              type: "text",
              text: "No app specified and no current app. Provide app_id or launch an app first.",
            },
          ],
          details: {},
          isError: true,
        };
      }

      const timeoutMs = Math.min(params.timeout_seconds || 30, 120) * 1000;

      // Optionally clear the buffer
      if (params.clear_buffer !== false) {
        clearEventBuffer();
      }

      // Check the buffer first for any already-arrived response
      const bufferedResponse = getBufferedEvent("user_response");
      if (bufferedResponse && bufferedResponse.type === "user_response") {
        if (!appId || bufferedResponse.app_id === appId) {
          clearEventBuffer();
          return {
            content: [
              {
                type: "text",
                text: JSON.stringify(
                  {
                    app_id: bufferedResponse.app_id,
                    response: bufferedResponse.response,
                    source: "buffered",
                  },
                  null,
                  2,
                ),
              },
            ],
            details: {
              app_id: bufferedResponse.app_id,
              response: bufferedResponse.response,
            },
          };
        }
      }

      // Wait for the response
      const result = await new Promise<{
        app_id: string;
        response: string;
      }>((resolve, reject) => {
        const timeout = setTimeout(() => {
          unsub();
          reject(
            new Error(
              `Timed out after ${timeoutMs / 1000}s waiting for response from '${appId || "any"}'.`,
            ),
          );
        }, timeoutMs);
        let active = true;
        const unsub = onMessage((msg: HarnessMessage) => {
          if (!active) return;
          if (msg.type === "user_response") {
            if (!appId || msg.app_id === appId) {
              clearTimeout(timeout);
              active = false;
              unsub();
              resolve({
                app_id: msg.app_id,
                response: msg.response,
              });
            }
          }
        });
      });

      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(
              {
                app_id: result.app_id,
                response: result.response,
                source: "live",
              },
              null,
              2,
            ),
          },
        ],
        details: {
          app_id: result.app_id,
          response: result.response,
        },
      };
    },
  });
}
