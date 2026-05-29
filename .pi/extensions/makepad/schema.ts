// Wire format matches Rust autosurgeon enum serialization:
// each variant is an object with a single key equal to the variant name.
export type AgentRequest =
  | { LaunchApp: { id: string; splash_body: string } }
  | { CloseApp: { id: string } }
  | { Inference: { content: string; app_id: string } };

export type AgentResponse =
  | { AppLaunched: { id: string } }
  | { AppClosed: { id: string } }
  | { InferenceResult: { app_id: string; content: string } }
  | { Chat: string };

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
