// Wire format matches Rust autosurgeon enum serialization.
export type AppStatus = "Pending" | "Launched";

export interface PendingApp {
  id: string;
  splash_body: string;
  status: AppStatus;
}

export interface AgentDoc {
  pending_app: PendingApp | null;
  extension_requests: boolean;
  should_exit: boolean;
}
