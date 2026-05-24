import { get } from "./client";

export interface SessionSummary {
  session_id: string;
  workspace: string;
  title: string;
  created_at: string;
  turn_count: number;
  status: "Active" | "Done" | "Interrupted";
  mtime: string;
  size: number;
}

export type StoredEventItem = {
  id: number;
  payload: Record<string, unknown>;
};

export type EventsResponse =
  | StoredEventItem[]
  | {
      events: StoredEventItem[];
      active_stream?: Array<Record<string, unknown>>;
    };

export function fetchSessions(
  workspace?: string,
): Promise<{ ok: boolean; sessions: SessionSummary[] }> {
  const qs = workspace ? `?workspace=${encodeURIComponent(workspace)}` : "";
  return get(`/sessions${qs}`);
}

export function fetchSessionEvents(
  sessionId: string,
  workspace: string,
): Promise<EventsResponse> {
  const qs = `?workspace=${encodeURIComponent(workspace)}`;
  return get(`/sessions/${sessionId}/events${qs}`);
}
