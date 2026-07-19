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
  _workspace?: string,
): Promise<{ ok: boolean; sessions: SessionSummary[] }> {
  return Promise.reject(new Error("legacy Session API removed"));
}

export function fetchSessionEvents(
  _sessionId: string,
  _workspace: string,
): Promise<EventsResponse> {
  return Promise.reject(new Error("legacy Session API removed"));
}
