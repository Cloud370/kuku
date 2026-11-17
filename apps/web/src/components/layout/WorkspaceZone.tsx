import type { SubmitEvent } from "react";
import { cn } from "@/lib/cn";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";

export interface WorkspaceZoneProps {
  workspace?: string;
  workspaces?: string[];
  onSelect?: (workspace: string) => void;
  error?: string;
}

export function WorkspaceZone({
  workspace = "",
  workspaces = [],
  onSelect,
  error,
}: WorkspaceZoneProps) {
  function select(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    const value = new FormData(event.currentTarget).get("workspace");
    if (typeof value === "string" && value.trim()) onSelect?.(value.trim());
  }

  return (
    <div className="h-full flex flex-col p-3 gap-2 overflow-auto">
      <form className="flex gap-1 shrink-0" onSubmit={select}>
        <Input
          key={workspace}
          aria-label="Workspace"
          name="workspace"
          placeholder="Server workspace path"
          defaultValue={workspace}
        />
        <Button type="submit" variant="secondary" size="sm">
          Open
        </Button>
      </form>
      {error && (
        <p className="text-xs text-red-400" role="alert">
          {error}
        </p>
      )}
      <div className="flex-1 flex flex-col gap-1.5">
        {workspaces.map((path) => (
          <button
            key={path}
            onClick={() => {
              onSelect?.(path);
            }}
            className={cn(
              "text-left px-3 py-1.5 text-[var(--text-sm)] rounded-[var(--radius-md)] transition-colors cursor-pointer truncate",
              path === workspace
                ? "bg-[var(--color-accent)] text-white font-medium"
                : "text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]",
            )}
            title={path}
          >
            {path}
          </button>
        ))}
      </div>
    </div>
  );
}
