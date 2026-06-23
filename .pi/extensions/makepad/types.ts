// ── Local extension state ────────────────────────────────────────────────

export type AppStatus = "Pending" | "Launched" | "Error" | "Closed";

export interface AppState {
  app_id: string;
  status: AppStatus;
  splash_body: string;
  /** Last error message from rendering failure, if any */
  last_error?: string;
}

// ── JSON WS protocol types (pi ↔ harness) ───────────────────────────────

export interface WelcomeMessage {
  type: "welcome";
}

export interface StatusMessage {
  type: "status";
  app_id: string;
  status: "Pending" | "Launched";
}

export interface UserResponseMessage {
  type: "user_response";
  app_id: string;
  response: string;
}

export interface ErrorMessage {
  type: "error";
  app_id: string;
  message: string;
}

export interface DebugResponseMessage {
  type: "debug_response";
  app_id: string;
  result: string;
}

export interface DocStateMessage {
  type: "doc_state";
  app_id: string | null;
  user_response: string | null;
  pi_response: string | null;
  error_message: string | null;
  status: string | null;
}

export type HarnessMessage =
  | WelcomeMessage
  | StatusMessage
  | UserResponseMessage
  | ErrorMessage
  | DebugResponseMessage
  | DocStateMessage;

// ── Pi → Harness message types ─────────────────────────────────────────

export interface GetDocMessage {
  type: "get_doc";
}
