import type { Target } from "@/api/sessions";

export function sessionHref({ workspace, session, agent }: Target) {
  return `/?${new URLSearchParams({ workspace, session, agent })}`;
}

export function workspaceHref(workspace: string) {
  return workspace ? `/?${new URLSearchParams({ workspace })}` : "/";
}

export function draftHref(workspace: string) {
  return `/session/new?${new URLSearchParams({ workspace })}`;
}
