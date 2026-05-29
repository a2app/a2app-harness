import { Repo, type DocHandle } from "@automerge/automerge-repo";
import { WebSocketClientAdapter } from "@automerge/automerge-repo-network-websocket";

import type { AgentDoc } from "./schema";

const HARNESS_WS = "ws://127.0.0.1:2341/sync";
const HARNESS_DOC_ID_URL = "http://127.0.0.1:2348/doc_id";
const CONNECT_TIMEOUT_MS = 15_000;
const POLL_INTERVAL_MS = 500;
const FIND_RETRY_MS = 200;

let handle: DocHandle<AgentDoc> | null = null;

export async function connectToHarness(): Promise<DocHandle<AgentDoc>> {
  const adapter = new WebSocketClientAdapter(HARNESS_WS);
  const repo = new Repo({ network: [adapter] });

  const docIdStr = await pollDocId(
    HARNESS_DOC_ID_URL,
    CONNECT_TIMEOUT_MS,
    POLL_INTERVAL_MS,
  );
  handle = await findHarnessDoc(repo, docIdStr, CONNECT_TIMEOUT_MS);
  await handle.whenReady();
  return handle;
}

async function findHarnessDoc(
  repo: Repo,
  rawDocId: string,
  timeoutMs: number,
): Promise<DocHandle<AgentDoc>> {
  const deadline = Date.now() + timeoutMs;
  const candidates = [rawDocId, `automerge:${rawDocId}`];
  let lastError: unknown = null;

  while (Date.now() <= deadline) {
    for (const candidate of candidates) {
      try {
        return await repo.find<AgentDoc>(candidate as any);
      } catch (err) {
        lastError = err;
      }
    }

    await new Promise((resolve) => setTimeout(resolve, FIND_RETRY_MS));
  }

  throw new Error(
    `Timed out finding harness document '${rawDocId}': ${String(lastError)}`,
  );
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
      const doc = docHandle.doc();
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
