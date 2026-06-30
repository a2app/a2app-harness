import { Type } from "typebox";
import type { AgentSession, ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { getModel } from "@earendil-works/pi-ai";
import { onMessage, sendToHarness } from "./doc-bridge.js";
import { tmpdir } from "node:os";
import { mkdirSync, existsSync } from "node:fs";
import { join } from "node:path";

// ── Blank-Slate Resource Loader ─────────────────────────────────────────
//
// Sub-agent sessions created via ai:init: or launch_app_with_agent must
// NOT inherit the main agent's system prompt, AGENTS.md, skills, or any
// other context. The splash app provides its own system prompt via
// ai:init:<prompt>, and the session should otherwise be blank.
//
// createAgentSession() defaults to DefaultResourceLoader which loads
// AGENTS.md from cwd → ancestors → ~/.pi/agent, plus SYSTEM.md, skills,
// prompt templates, extensions, etc. We override all of these to produce
// a completely blank slate.

let _blankLoader: any = null;

async function getBlankSlateResourceLoader(): Promise<any> {
  if (_blankLoader) return _blankLoader;

  const { DefaultResourceLoader, SettingsManager } =
    await import("@earendil-works/pi-coding-agent");

  // Use a temporary empty directory as agentDir so no config files leak in.
  const blankDir = join(tmpdir(), "pi-blank-agent-" + process.pid);
  if (!existsSync(blankDir)) {
    mkdirSync(blankDir, { recursive: true });
  }

  const settingsManager = SettingsManager.create(blankDir, blankDir);

  _blankLoader = new DefaultResourceLoader({
    cwd: blankDir,
    agentDir: blankDir,
    settingsManager,
    noContextFiles: true,
    noSkills: true,
    noPromptTemplates: true,
    noThemes: true,
    noExtensions: true,
    // Force system prompt to empty — never inherit the main agent's prompt
    systemPromptOverride: () => "",
    // Force agents files (AGENTS.md, CLAUDE.md, etc.) to empty
    agentsFilesOverride: () => ({ agentsFiles: [] }),
  });
  await _blankLoader.reload();

  return _blankLoader;
}

// ── Session Store ─────────────────────────────────────────────────────────
// Active background agent sessions, persisted across tool calls within the
// same extension lifetime. Sessions are cleaned up on session shutdown.

interface StoredSession {
  session: AgentSession;
  provider: string;
  modelId: string;
  createdAt: number;
}

const sessions = new Map<string, StoredSession>();
// Maps app_id → session_id so the auto-handler can route messages
const appSessionMap = new Map<string, string>();

// ── Helpers ───────────────────────────────────────────────────────────────

function getDefaultModel(): { provider: string; modelId: string } {
  return { provider: "deepseek", modelId: "deepseek-v4-flash" };
}

// ── Tool Registration ────────────────────────────────────────────────────

export function registerBackgroundAgentTools(pi: ExtensionAPI): void {
  // ── start_background_session ─────────────────────────────────────────
  pi.registerTool({
    name: "start_background_session",
    label: "Start Background Agent Session",
    description:
      "Create a new background agent sub-session using the pi SDK. Returns a session_id for subsequent interaction.",
    parameters: Type.Object({
      system_prompt: Type.Optional(
        Type.String({
          description: "Optional system prompt for the sub-agent",
        }),
      ),
      provider: Type.Optional(
        Type.String({
          description: "Model provider (e.g. 'anthropic', 'openai')",
        }),
      ),
      model_id: Type.Optional(
        Type.String({
          description: "Model ID (e.g. 'claude-sonnet-4-6', 'gpt-4o')",
        }),
      ),
      thinking_level: Type.Optional(
        Type.String({
          description:
            "Thinking level: 'off', 'minimal', 'low', 'medium', 'high', 'xhigh'",
        }),
      ),
    }),
    async execute(_id: string, params: any, _signal: AbortSignal, _onUpdate: any) {
      try {
        const provider = params.provider || getDefaultModel().provider;
        const modelId = params.model_id || getDefaultModel().modelId;

        const model = getModel(provider as any, modelId as any);
        if (!model) {
          return {
            content: [{ type: "text", text: `Model not found: ${provider}/${modelId}.` }],
            details: {},
            isError: true,
          } as any;
        }

        const { AuthStorage, ModelRegistry, SessionManager, createAgentSession } =
          await import("@earendil-works/pi-coding-agent");

        const authStorage = AuthStorage.create();
        const modelsPath = process.env.HOME + "/.pi/agent/models.json";
        const modelRegistry = ModelRegistry.create(authStorage, modelsPath);

        const systemPrompt =
          params.system_prompt ||
          "You are a helpful background AI assistant. Be concise and accurate.";

        // Use blank-slate resource loader so sub-agent doesn't inherit
        // main agent's system prompt, AGENTS.md, or skills.
        const resourceLoader = await getBlankSlateResourceLoader();

        const { session } = await createAgentSession({
          model,
          thinkingLevel: params.thinking_level || "off",
          authStorage,
          modelRegistry,
          sessionManager: SessionManager.inMemory(),
          resourceLoader,
          tools: [],
        });

        // Collect output via subscription
        const unsub = session.subscribe((event: any) => {
          if (
            event.type === "message_update" &&
            event.assistantMessageEvent?.type === "text_delta"
          ) {
            // Just keeping the subscription alive
          }
        });

        const sessionId = `bg-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
        sessions.set(sessionId, { session, provider, modelId, createdAt: Date.now() });

        return {
          content: [
            {
              type: "text",
              text: JSON.stringify(
                {
                  session_id: sessionId,
                  message: "Background agent session created. Use send_background_message to interact.",
                  model: `${provider}/${modelId}`,
                  system_prompt: systemPrompt,
                },
                null,
                2,
              ),
            },
          ],
          details: { session_id: sessionId, model: `${provider}/${modelId}` },
        } as any;
      } catch (err) {
        return {
          content: [{ type: "text", text: `Failed to create background session: ${err}` }],
          details: { error: String(err) },
          isError: true,
        } as any;
      }
    },
  });

  // ── send_background_message ──────────────────────────────────────────
  pi.registerTool({
    name: "send_background_message",
    label: "Send Background Message",
    description:
      "Send a prompt to an existing background agent sub-session and get the response. History is maintained across calls.",
    parameters: Type.Object({
      session_id: Type.String({ description: "Session ID from start_background_session" }),
      message: Type.String({ description: "The prompt to send to the sub-agent" }),
      max_wait_seconds: Type.Optional(
        Type.Number({ description: "Max seconds to wait (default 30, max 120)" }),
      ),
    }),
    async execute(_id: string, params: any, _signal: AbortSignal, _onUpdate: any) {
      const stored = sessions.get(params.session_id);
      if (!stored) {
        return {
          content: [{ type: "text", text: `Session '${params.session_id}' not found.` }],
          details: {},
          isError: true,
        } as any;
      }

      try {
        const maxWait = Math.min(params.max_wait_seconds || 30, 120) * 1000;

        // Collect response
        let fullResponse = "";
        const unsub = stored.session.subscribe((event: any) => {
          if (
            event.type === "message_update" &&
            event.assistantMessageEvent?.type === "text_delta"
          ) {
            fullResponse += event.assistantMessageEvent.delta;
          }
        });

        await Promise.race([
          stored.session.prompt(params.message, { expandPromptTemplates: false }),
          new Promise((_, reject) =>
            setTimeout(() => reject(new Error(`Timed out after ${maxWait / 1000}s`)), maxWait),
          ),
        ]);

        unsub();

        return {
          content: [{ type: "text", text: fullResponse || "(empty response)" }],
          details: { session_id: params.session_id, response_length: fullResponse.length },
        } as any;
      } catch (err) {
        return {
          content: [{ type: "text", text: `Background agent error: ${err}` }],
          details: { error: String(err), session_id: params.session_id },
          isError: true,
        } as any;
      }
    },
  });

  // ── list_background_sessions ─────────────────────────────────────────
  pi.registerTool({
    name: "list_background_sessions",
    label: "List Background Sessions",
    description:
      "List all active background agent sub-sessions with metadata (session_id, model, created_at).",
    parameters: Type.Object({}),
    async execute() {
      if (sessions.size === 0) {
        return {
          content: [{ type: "text", text: "No active background sessions." }],
          details: { sessions: [] },
        } as any;
      }

      const sessionList = Array.from(sessions.entries()).map(([id, stored]) => ({
        session_id: id,
        model: `${stored.provider}/${stored.modelId}`,
        created_at: new Date(stored.createdAt).toISOString(),
      }));

      return {
        content: [{ type: "text", text: JSON.stringify(sessionList, null, 2) }],
        details: { sessions: sessionList },
      } as any;
    },
  });

  // ── send_pi_response ────────────────────────────────────────────────
  //
  // Sends data from pi to the running splash app.
  // Writes to the shared CRDT doc's pi_response field, which the
  // makepad-host reads and sets on the __pi_data label so the splash
  // app can read it via ui.__pi_data.text().
  //
  pi.registerTool({
    name: "send_pi_response",
    label: "Send Pi Response",
    description:
      "Send data from pi to the running Splash mini-app. The splash app reads this via ui.__pi_data.text() inside on_click handlers.",
    parameters: Type.Object({
      app_id: Type.String({ description: "App ID to send data to" }),
      data: Type.String({ description: "The data to send to the splash app" }),
    }),
    async execute(_id: string, params: any) {
      try {
        const { sendToHarness } = await import("./doc-bridge.js");
        const { ensureConnected } = await import("./tools.js");
        await ensureConnected();
        sendToHarness({ type: "send_pi_response", app_id: params.app_id, data: params.data });
        return {
          content: [{ type: "text", text: `Sent ${params.data.length} chars to app '${params.app_id}'.` }],
          details: { app_id: params.app_id, bytes: params.data.length },
        } as any;
      } catch (err) {
        return {
          content: [{ type: "text", text: `Failed to send pi response: ${err}` }],
          details: { error: String(err) },
          isError: true,
        } as any;
      }
    },
  });

  // ── launch_app_with_agent ────────────────────────────────────────────
  //
  // Launches a Makepad splash app with an automatically-started background
  // sub-agent session. The splash app communicates with the sub-agent via:
  //   - ui.__pi_response.set_text("ai:ask:<message>")  to send a message
  //   - ui.__pi_data.text() to read the response
  //
  // The sub-agent is created automatically and associated with this app.
  //
  pi.registerTool({
    name: "launch_app_with_agent",
    label: "Launch App with Agent",
    description:
      "Launch a Makepad splash app with an automatically-started background sub-agent session. The splash app uses ui.__pi_response.set_text('ai:ask:<message>') to send messages to the sub-agent and ui.__pi_data.text() to read responses inside on_click handlers. No session management needed in the splash body.",
    parameters: Type.Object({
      app_id: Type.String({ description: "Unique ID for this app instance" }),
      splash_body: Type.String({ description: "Makepad Splash body string" }),
      system_prompt: Type.Optional(
        Type.String({ description: "Optional system prompt for the sub-agent" }),
      ),
      agent_model: Type.Optional(
        Type.String({ description: "Optional model for the sub-agent (e.g. 'deepseek-v4-flash')" }),
      ),
    }),
    async execute(_id: string, params: any, _signal: AbortSignal, _onUpdate: any) {
      try {
        // 1. Launch the splash app via harness
        const { ensureConnected } = await import("./tools.js");
        // We reuse the connection setup from tools
        await ensureConnected();
        sendToHarness({
          type: "launch",
          app_id: params.app_id,
          splash_body: params.splash_body,
        });

        // Wait for launch confirmation
        const launchResult = await new Promise<{ ok: boolean; message: string }>((resolve) => {
          const timeout = setTimeout(() =>
            resolve({ ok: false, message: `Timed out awaiting confirmation for '${params.app_id}'.` }), 12_000);
          let statusReceived = false;
          let statusTimer: ReturnType<typeof setTimeout> | null = null;
          let active = true;
          const unsub = onMessage((msg: any) => {
            if (!active) return;
            if (msg.type === "error" && msg.app_id === params.app_id) {
              clearTimeout(timeout);
              if (statusTimer) clearTimeout(statusTimer);
              active = false; unsub();
              resolve({ ok: false, message: `Render error: ${msg.message}` });
            }
            if (msg.type === "status" && msg.status === "Launched" && msg.app_id === params.app_id && !statusReceived) {
              statusReceived = true;
              statusTimer = setTimeout(() => {
                clearTimeout(timeout);
                active = false; unsub();
                resolve({ ok: true, message: `App '${params.app_id}' launched.` });
              }, 1500);
            }
          });
        });

        if (!launchResult.ok) {
          return { content: [{ type: "text", text: launchResult.message }], details: {}, isError: true } as any;
        }

        // 2. Start background sub-agent session
        const modelId = params.agent_model || "deepseek-v4-flash";
        const { getModel } = await import("@earendil-works/pi-ai");
        const model = getModel("deepseek", modelId as any);
        if (!model) {
          return { content: [{ type: "text", text: `Model not found: deepseek/${modelId}` }], details: {}, isError: true } as any;
        }

        const { AuthStorage, ModelRegistry, SessionManager, createAgentSession } =
          await import("@earendil-works/pi-coding-agent");
        const authStorage = AuthStorage.create();
        const modelRegistry = ModelRegistry.create(authStorage, process.env.HOME + "/.pi/agent/models.json");
        const resourceLoader = await getBlankSlateResourceLoader();

        const systemPrompt = params.system_prompt ||
          "You are a background AI assistant powering a native UI app. Be concise.";

        const { session } = await createAgentSession({
          model,
          thinkingLevel: "off",
          authStorage,
          modelRegistry,
          sessionManager: SessionManager.inMemory(),
          resourceLoader,
          tools: [],
        });

        const sessionId = "bg-" + Date.now() + "-" + Math.random().toString(36).slice(2, 8);
        sessions.set(sessionId, {
          session,
          provider: "deepseek",
          modelId,
          createdAt: Date.now(),
        });

        // Associate app_id with session_id for the auto-handler
        appSessionMap.set(params.app_id, sessionId);

        // Seed the system prompt as the first message and WAIT for it.
        // Using fire-and-forget causes race conditions with the first ai:ask.
        if (systemPrompt) {
          await session.prompt("[SYSTEM CONTEXT] " + systemPrompt, { expandPromptTemplates: false });
        }

        return {
          content: [{
            type: "text",
            text: JSON.stringify({
              app_id: params.app_id,
              session_id: sessionId,
              message: "App launched with background agent. Use ai:ask:<message> protocol via __pi_response/__pi_data.",
              protocol: "Send: ui.__pi_response.set_text('ai:ask:<msg>')\nRead: ui.__pi_data.text() inside on_click",
            }, null, 2),
          }],
          details: { app_id: params.app_id, session_id: sessionId },
        } as any;
      } catch (err) {
        return { content: [{ type: "text", text: `Failed: ${err}` }], details: { error: String(err) }, isError: true } as any;
      }
    },
  });

  // ── stop_background_session ──────────────────────────────────────────
  pi.registerTool({
    name: "stop_background_session",
    label: "Stop Background Session",
    description:
      "Stop and dispose a background agent sub-session. Frees resources and removes it from the active list.",
    parameters: Type.Object({
      session_id: Type.String({ description: "Session ID to stop" }),
    }),
    async execute(_id: string, params: any) {
      const stored = sessions.get(params.session_id);
      if (!stored) {
        return {
          content: [{ type: "text", text: `Session '${params.session_id}' not found or already stopped.` }],
          details: {},
        } as any;
      }

      try {
        stored.session.dispose();
      } catch {
        // Ignore
      }
      sessions.delete(params.session_id);

      return {
        content: [{ type: "text", text: `Session '${params.session_id}' stopped and disposed.` }],
        details: { session_id: params.session_id },
      } as any;
    },
  });
}

// ── Auto Background Handler ─────────────────────────────────────────────
//
// Listens for harness messages and automatically handles ai:* patterns:
//   ai:start          → creates a background AgentSession
//   ai:ask:<sid>:<msg> → sends message to session, forwards response
//
// This makes the ai-chat app work without pi agent involvement.
//

let autoHandlerStarted = false;

function getCurrentAppId(): string {
  // Try to get the current app_id from the tools module
  try {
    // The tools module tracks currentApp - we use a simple fallback
    return "";
  } catch {
    return "";
  }
}

async function initSession(appId: string, initialMessage?: string): Promise<string | null> {
  try {
    const { AuthStorage, ModelRegistry, SessionManager, createAgentSession } =
      await import("@earendil-works/pi-coding-agent");
    const { getModel } = await import("@earendil-works/pi-ai");

    const model = getModel("deepseek", "deepseek-v4-flash");
    if (!model) return null;

    const authStorage = AuthStorage.create();
    const modelsPath = process.env.HOME + "/.pi/agent/models.json";
    const modelRegistry = ModelRegistry.create(authStorage, modelsPath);
    const resourceLoader = await getBlankSlateResourceLoader();

    const { session } = await createAgentSession({
      model,
      thinkingLevel: "off",
      authStorage,
      modelRegistry,
      sessionManager: SessionManager.inMemory(),
      resourceLoader,
      tools: [],
    });

    const sessionId = "bg-" + Date.now() + "-" + Math.random().toString(36).slice(2, 8);
    sessions.set(sessionId, {
      session,
      provider: "deepseek",
      modelId: "deepseek-v4-flash",
      createdAt: Date.now(),
    });

    // If there's an initial message, process it immediately
    if (initialMessage) {
      let response = "";
      const unsub = session.subscribe((event: any) => {
        if (event.type === "message_update" && event.assistantMessageEvent?.type === "text_delta") {
          response += event.assistantMessageEvent.delta;
        }
      });
      await session.prompt(initialMessage, { expandPromptTemplates: false });
      unsub();
      sendToHarness({ type: "send_pi_response", app_id: appId, data: sessionId + "|" + (response || "") });
    } else {
      sendToHarness({ type: "send_pi_response", app_id: appId, data: sessionId });
    }

    return sessionId;
  } catch (err) {
    console.error("[bg-agent] Failed to start session:", err);
    return null;
  }
}

function handleAutoMessage(data: string, appId: string): void {
  // ── ai:init:<system_prompt> ────────────────────────────────────────
  // App provides its own system prompt for the sub-agent session.
  // If a session already exists for this app, it is disposed and replaced
  // with a new one using the app-provided system prompt.
  if (data.startsWith("ai:init:")) {
    const systemPrompt = data.slice(8); // remove "ai:init:"
    (async () => {
      try {
        // Dispose existing session for this app if any
        const existingSid = appSessionMap.get(appId);
        if (existingSid) {
          const existing = sessions.get(existingSid);
          if (existing) {
            try { existing.session.dispose(); } catch {}
          }
          sessions.delete(existingSid);
        }

        // Create new session with the app-provided system prompt.
        // Use a blank-slate resource loader so the sub-agent does NOT
        // inherit the main agent's system prompt, AGENTS.md, or skills.
        const { AuthStorage, ModelRegistry, SessionManager, createAgentSession } =
          await import("@earendil-works/pi-coding-agent");
        const { getModel } = await import("@earendil-works/pi-ai");

        const model = getModel("deepseek", "deepseek-v4-flash");
        if (!model) {
          console.error("[bg-agent] Model not found for init");
          return;
        }

        const authStorage = AuthStorage.create();
        const modelRegistry = ModelRegistry.create(authStorage, process.env.HOME + "/.pi/agent/models.json");
        const resourceLoader = await getBlankSlateResourceLoader();

        const { session } = await createAgentSession({
          model,
          thinkingLevel: "off",
          authStorage,
          modelRegistry,
          sessionManager: SessionManager.inMemory(),
          resourceLoader,
          tools: [],
        });

        const sessionId = "bg-" + Date.now() + "-" + Math.random().toString(36).slice(2, 8);
        sessions.set(sessionId, {
          session,
          provider: "deepseek",
          modelId: "deepseek-v4-flash",
          createdAt: Date.now(),
        });
        appSessionMap.set(appId, sessionId);

        // Send the app-provided system prompt as the first message to set context.
        // The agent session doesn't expose a systemPrompt option via createAgentSession,
        // so we seed the conversation with a context-setting message.
        if (systemPrompt) {
          let _initResp = "";
          const _unsub = session.subscribe((event: any) => {
            if (event.type === "message_update" && event.assistantMessageEvent?.type === "text_delta") {
              _initResp += event.assistantMessageEvent.delta;
            }
          });
          await session.prompt("[SYSTEM CONTEXT] " + systemPrompt, { expandPromptTemplates: false });
          _unsub();
        }

        // Confirm to the splash app that the session was initialized
        sendToHarness({ type: "send_pi_response", app_id: appId, data: "[Session initialized with app-provided system prompt]" });
        console.log("[bg-agent] Session initialized for app", appId, "with system prompt:", systemPrompt);
      } catch (err) {
        console.error("[bg-agent] Error initializing session:", err);
        sendToHarness({ type: "send_pi_response", app_id: appId, data: "[Error initializing session: " + err + "]" });
      }
    })();
    return;
  }

  // ── ai:ask:<message> ───────────────────────────────────────────────
  // Each delta is sent immediately. The makepad-host uses an mpsc channel
  // to deliver deltas individually (like AgentEvent::TextDelta) so rapid
  // sends don't coalesce — each one triggers a separate render update.
  if (data.startsWith("ai:ask:")) {
    const message = data.slice(7);
    if (!message) return;

    (async () => {
      try {
        let sid: string | null | undefined = appSessionMap.get(appId);
        if (!sid) {
          sid = await initSession(appId, undefined);
          if (!sid) return;
        }

        const stored = sessions.get(sid);
        if (!stored) return;

        let response = "";
        const unsub = stored.session.subscribe((event: any) => {
          if (event.type === "message_update" && event.assistantMessageEvent?.type === "text_delta") {
            response += event.assistantMessageEvent.delta;
            sendToHarness({ type: "send_streaming_delta", app_id: appId, delta: response });
          }
        });

        await stored.session.prompt(message, { expandPromptTemplates: false });
        unsub();

        sendToHarness({ type: "send_streaming_end", app_id: appId, final_text: response || "[No response]" });
      } catch (err) {
        console.error("[bg-agent] Error processing message:", err);
      }
    })();
  }
}

/**
 * Start the auto-background handler that listens for ai:* messages
 * from the harness. Safe to call multiple times.
 */
export function startAutoBackgroundHandler(): void {
  if (autoHandlerStarted) return;  // Only register once
  autoHandlerStarted = true;
  try {
    onMessage((msg: any) => {
      if (msg.type === "user_response") {
        const text = msg.response || "";
        if (text.startsWith("ai:")) {
          handleAutoMessage(text, msg.app_id || "");
        }
      }
    });
    console.log("[bg-agent] Auto background handler started");
  } catch (err) {
    console.error("[bg-agent] Failed to start auto handler:", err);
  }
}

// ── App-to-Session Mapping ──────────────────────────────────────────────
// Allows the launch_makepad_app tool to associate an app with a session

export function registerAppSessionAssociation(appId: string, sessionId: string): void {
  appSessionMap.set(appId, sessionId);
  // Also re-register the onMessage handler to ensure it's active
  // (handlers might get lost after WS reconnection)
  startAutoBackgroundHandler();
}

// ── Cleanup ──────────────────────────────────────────────────────────────

export function disposeAllSessions(): void {
  for (const [, stored] of sessions) {
    try {
      stored.session.dispose();
    } catch {
      // ignore
    }
  }
  sessions.clear();
}
