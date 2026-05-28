import { getDocHandle } from "./doc-bridge.js";
import type { AgentDoc } from "./schema.js";

type ExtensionAPI = any;

type InferenceRequest = {
  type: "Inference";
  content: string;
  app_id: string;
};

export function watchInferenceRequests(pi: ExtensionAPI): ReturnType<typeof setInterval> {
  const docHandle = getDocHandle();

  const poll = async (): Promise<void> => {
    const doc = docHandle.docSync();
    if (!doc) {
      return;
    }

    const inferenceReq = doc.requests.find(
      (r): r is InferenceRequest => r.type === "Inference",
    );
    if (!inferenceReq) {
      return;
    }

    docHandle.change((d) => {
      const idx = d.requests.findIndex(
        (r) => r.type === "Inference" && r.app_id === inferenceReq.app_id,
      );
      if (idx !== -1) {
        d.requests.splice(idx, 1);
      }
    });

    const response = await callPiInference(pi, inferenceReq.content, doc);

    docHandle.change((d) => {
      d.responses.push({
        type: "InferenceResult",
        app_id: inferenceReq.app_id,
        content: response,
      });
    });
  };

  return setInterval(() => {
    void poll();
  }, 200);
}

async function callPiInference(
  pi: ExtensionAPI,
  prompt: string,
  doc: AgentDoc,
): Promise<string> {
  if (typeof pi.sendUserMessage !== "function") {
    return "Inference API unavailable in this Pi runtime.";
  }

  const runningApps = Object.keys(doc.mini_apps);
  const prefix =
    runningApps.length > 0
      ? `Running apps: ${runningApps.join(", ")}. Return only the requested result.\n\n`
      : "";

  const text = `${prefix}${prompt}`;

  try {
    const direct = await pi.sendUserMessage(text, { deliverAs: "followUp" });
    if (typeof direct === "string" && direct.trim().length > 0) {
      return direct;
    }
    if (direct && typeof direct.text === "string" && direct.text.trim().length > 0) {
      return direct.text;
    }
  } catch (err) {
    if (typeof pi.sendMessage === "function") {
      await pi.sendMessage({
        role: "system",
        content: `Inference follow-up failed: ${String(err)}`,
      });
    }
  }

  return "Inference call did not return a response.";
}
