import { useQuery } from "@tanstack/react-query";
import { fetchAgents, fetchSessions, fetchWorkspaces, resolveWorkspace } from "@/api/sessions";
import type { Target } from "@/api/sessions";

export function useSessions(workspace: string, search: string) {
  return useQuery({
    queryKey: ["sessions", workspace, search],
    queryFn: ({ signal }) => fetchSessions(workspace, search, signal),
    enabled: workspace.length > 0,
    staleTime: 5_000,
  });
}

export function useWorkspaces() {
  return useQuery({
    queryKey: ["workspaces"],
    queryFn: ({ signal }) => fetchWorkspaces(signal),
    staleTime: 10_000,
  });
}

export function useWorkspaceResolution(workspace: string) {
  return useQuery({
    queryKey: ["workspace", workspace],
    queryFn: ({ signal }) => resolveWorkspace(workspace, signal),
    enabled: workspace.length > 0,
    retry: false,
    staleTime: 30_000,
  });
}

export function useAgents(target: Target, enabled: boolean) {
  return useQuery({
    queryKey: ["agents", target.workspace, target.session],
    queryFn: ({ signal }) => fetchAgents(target, signal),
    enabled,
    retry: false,
  });
}
