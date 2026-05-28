import { Repo, type DocHandle } from "@automerge/automerge-repo";
import { WebSocketClientAdapter } from "@automerge/automerge-repo-network-websocket";

import type { AgentDoc } from "./schema.js";

const HARNESS_WS = "ws://127.0.0.1:2341";
const HARNESS_DOC_ID_URL = "http://127.0.0.1:2348/doc_id";
const CONNECT_TIMEOUT_MS = 15_000;
const POLL_INTERVAL_MS = 500;

let handle: DocHandle<AgentDoc> | null = null;

export async function connectToHarness(): Promise<DocHandle<AgentDoc>> {
  const adapter = new WebSocketClientAdapter(HARNESS_WS);
  const repo = new Repo({ network: [adapter] });

  const docIdStr = await pollDocId(
    HARNESS_DOC_ID_URL,
    CONNECT_TIMEOUT_MS,
    POLL_INTERVAL_MS,
  );
  handle = await repo.find<AgentDoc>(docIdStr as any);
  await handle.whenReady();
  return handle;
}

export function getDocHandle(): DocHandle<AgentDoc> {
  if (!handle) {
    throw new Error("Not connected to harness");
  }
  return handle;
}

export async function waitForResponse<T extends AgentDoc["responses"][number]>(
  docHandle: DocHandle<AgentDoc>,
  predicate: (r: AgentDoc["responses"][number]) => r is T,
  signal?: AbortSignal,
  timeoutMs = 30_000,
): Promise<T | null> {
  return new Promise((resolve) => {
    const deadline = Date.now() + timeoutMs;

    const check = (): void => {
      const doc = docHandle.docSync();
      if (!doc) {
        setTimeout(check, 100);
        return;
      }

      const match = doc.responses.find(predicate);
      if (match) {
        docHandle.change((d) => {
          const idx = d.responses.findIndex(predicate);
          if (idx !== -1) {
            d.responses.splice(idx, 1);
          }
        });
        resolve(match);
        return;
      }

      if (signal?.aborted || Date.now() > deadline) {
        resolve(null);
        return;
      }

      setTimeout(check, 100);
    };

    check();
  });
}

async function pollDocId(
  url: string,
  timeoutMs: number,
  pollMs: number,
): Promise<string> {
  const deadline = Date.now() + timeoutMs;

  while (Date.now() <= deadline) {
    try {
      const resp = await fetch(url);
      if (resp.ok) {
        const text = (await resp.text()).trim();
        if (text.length > 0) {
          return text;
        }
      }
    } catch {
      // Harness may not be up yet.
    }

    await new Promise((resolve) => setTimeout(resolve, pollMs));
  }

  throw new Error(`Timed out waiting for harness doc id at ${url}`);
}
