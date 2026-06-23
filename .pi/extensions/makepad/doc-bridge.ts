import WebSocket from "ws";
import type { HarnessMessage } from "./types.js";

const HARNESS_WS = "ws://127.0.0.1:2341/";
const CONNECT_TIMEOUT_MS = 15_000;
const CONNECT_RETRY_MS = 500;
const QUICK_TIMEOUT_MS = 1_500;

// ── Connection state ─────────────────────────────────────────────────────

let ws: WebSocket | null = null;
let messageHandlers: Array<(msg: HarnessMessage) => void> = [];
let connectedResolve: (() => void) | null = null;
let connectedPromise: Promise<void> | null = null;

// ── Persistent event buffer ──────────────────────────────────────────────
//
// The event buffer accumulates incoming messages from the harness so that
// tools can inspect recent events even if they weren't listening at the
// moment the message arrived. This enables a "service worker" pattern where
// events are delivered asynchronously and can be processed by any tool.
//
// Buffer is keyed by message type, storing the most recent message of each
// type. This is intentionally NOT a queue — we only keep the latest value
// for each event type (user_response, error, status, etc.) because:
//   a) The CRDT doc state is the source of truth; these are just notifications
//   b) The harness sends many status/error updates; only the latest matters
//
// To clear: use clearEventBuffer()

const eventBuffer: Map<string, HarnessMessage> = new Map();

/** Get the latest message of a specific type from the buffer (or undefined) */
export function getBufferedEvent(type: string): HarnessMessage | undefined {
  return eventBuffer.get(type);
}

/** Get a copy of all buffered events */
export function getAllBufferedEvents(): HarnessMessage[] {
  return Array.from(eventBuffer.values());
}

/** Clear the event buffer */
export function clearEventBuffer(): void {
  eventBuffer.clear();
}

// ── Message handlers ────────────────────────────────────────────────────

export function onMessage(handler: (msg: HarnessMessage) => void): () => void {
  messageHandlers.push(handler);
  return () => {
    messageHandlers = messageHandlers.filter((h) => h !== handler);
  };
}

// ── Connection management ────────────────────────────────────────────────

export async function connectToHarness(): Promise<void> {
  if (connectedPromise) return connectedPromise;

  connectedPromise = new Promise<void>((resolve, reject) => {
    const deadline = Date.now() + CONNECT_TIMEOUT_MS;

    const tryConnect = () => {
      if (Date.now() > deadline) {
        connectedPromise = null;
        reject(new Error("Timed out connecting to harness"));
        return;
      }

      const socket = new WebSocket(HARNESS_WS);
      let settled = false;

      const cleanRetry = () => {
        if (settled) return;
        settled = true;
        socket.close();
        setTimeout(tryConnect, CONNECT_RETRY_MS);
      };

      socket.on("open", () => {
        ws = socket;
        connectedResolve = resolve;
      });

      socket.on("message", (data: Buffer) => {
        try {
          const msg = JSON.parse(data.toString()) as HarnessMessage;
          
          // Buffer the event (keyed by type) for later inspection
          eventBuffer.set(msg.type, msg);

          if (msg.type === "welcome") {
            if (connectedResolve) {
              connectedResolve();
              connectedResolve = null;
            }
          }

          // Notify all registered handlers
          // Each handler is individually wrapped so one error doesn't
          // block subsequent handlers (important after extension reloads).
          for (const handler of messageHandlers) {
            try {
              handler(msg);
            } catch {
              // Individual handler error — don't block other handlers
            }
          }
        } catch (err) {
          // ignore parse errors
        }
      });

      socket.on("close", () => {
        ws = null;
        cleanRetry();
      });

      socket.on("error", () => {
        socket.close();
        cleanRetry();
      });

      const connectTimeout = setTimeout(() => {
        if (ws?.readyState !== WebSocket.OPEN) {
          cleanRetry();
        }
      }, CONNECT_RETRY_MS);

      socket.on("open", () => {
        clearTimeout(connectTimeout);
      });
    };

    tryConnect();
  });

  return connectedPromise;
}

export async function quickConnectCheck(): Promise<boolean> {
  // Quick TCP-level check: does anything answer on the harness port?
  // We try a WebSocket connect with a short deadline.
  return new Promise<boolean>((resolve) => {
    const socket = new WebSocket(HARNESS_WS);
    const timer = setTimeout(() => {
      socket.close();
      resolve(false);
    }, QUICK_TIMEOUT_MS);
    socket.on("open", () => {
      clearTimeout(timer);
      socket.close();
      resolve(true);
    });
    socket.on("error", () => {
      clearTimeout(timer);
      resolve(false);
    });
  });
}

export function sendToHarness(msg: object): void {
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(msg));
  }
}
