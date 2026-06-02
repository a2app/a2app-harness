import { createAgentSession, SessionManager } from "@earendil-works/pi-coding-agent";
import { getDocHandle } from "./doc-bridge";
import type { AgentDoc } from "./schema";

type ExtensionAPI = any;

type InferenceRequest = {
  Inference: {
    content: string;
    app_id: string;
  };
};

export function watchInferenceRequests(pi: ExtensionAPI): ReturnType<typeof setInterval> {
  const docHandle = getDocHandle();

  const poll = async (): Promise<void> => {
    const doc = docHandle.doc();
    if (!doc) {
      return;
    }

    const inferenceReq = doc.requests.find(
      (r): r is InferenceRequest => "Inference" in r,
    );
    if (!inferenceReq) {
      return;
    }

    // Coerce RawString → plain string
    const content = String(inferenceReq.Inference.content);
    const app_id = String(inferenceReq.Inference.app_id);

    docHandle.change((d) => {
      const idx = d.requests.findIndex(
        (r) => "Inference" in r && String(r.Inference.app_id) === app_id,
      );
      if (idx !== -1) {
        d.requests.splice(idx, 1);
      }
    });

    const response = await callSubInference(pi, content, doc);

    // Write response via doc (direct sync)
    docHandle.change((d) => {
      d.responses.push({
        InferenceResult: {
          app_id,
          content: response,
        },
      });
    });

    // HTTP fallback: POST to harness endpoint which writes to stored_values
    try {
      await fetch(`http://127.0.0.1:2348/inference_response`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ app_id, content: response }),
      });
    } catch (err) {
      console.error(`[inference-loop] HTTP fallback error: ${err}`);
    }
  };

  return setInterval(() => {
    void poll();
  }, 200);
}

async function callSubInference(
  pi: ExtensionAPI,
  prompt: string,
  doc: AgentDoc,
): Promise<string> {
  try {
    const { session } = await createAgentSession({
      sessionManager: SessionManager.inMemory(),
      noTools: "all",
      model: pi.model,
    });

    let fullResponse = "";

    session.subscribe((event: any) => {
      if (
        event.type === "message_update" &&
        event.assistantMessageEvent?.type === "text_delta"
      ) {
        fullResponse += event.assistantMessageEvent.delta;
      }
    });

    await session.prompt(prompt);
    session.dispose();

    return fullResponse.trim().length > 0
      ? fullResponse
      : "(no response from sub-inference)";
  } catch (err) {
    return `Sub-inference error: ${String(err)}`;
  }
}
