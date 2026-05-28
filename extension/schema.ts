export type AgentRequest =
  | { type: "LaunchApp"; id: string; splash_body: string }
  | { type: "CloseApp"; id: string }
  | { type: "Inference"; content: string; app_id: string };

export type AgentResponse =
  | { type: "AppLaunched"; id: string }
  | { type: "AppClosed"; id: string }
  | { type: "InferenceResult"; app_id: string; content: string }
  | { type: "Chat"; value: string };

export interface MiniApp {
  splash_body: string;
  state: Record<string, string>;
}

export type ConversationFragment = { User: string } | { Assistant: string };

export interface StoredValue {
  value: string;
  description: string;
}

export interface AgentDoc {
  requests: AgentRequest[];
  responses: AgentResponse[];
  mini_apps: Record<string, MiniApp>;
  conversation_history: ConversationFragment[];
  stored_values: Record<string, StoredValue>;
  text_documents: Record<string, string>;
  active_document: string | null;
  should_exit: boolean;
  active_model: string | null;
}
