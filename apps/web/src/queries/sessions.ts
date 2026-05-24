import { useQuery } from "@tanstack/react-query";
import { fetchSessions, fetchSessionEvents } from "@/api/sessions";

export function useSessions(workspace?: string) {
  return useQuery({
    queryKey: ["sessions", workspace],
    queryFn: () => fetchSessions(workspace),
    staleTime: 10_000,
  });
}

export function useSessionEvents(
  sessionId: string | undefined,
  workspace: string,
) {
  return useQuery({
    queryKey: ["session-events", sessionId],
    queryFn: () => fetchSessionEvents(sessionId as string, workspace),
    enabled: !!sessionId,
  });
}
