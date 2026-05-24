import { useUIStore } from "@/stores/ui";
import { cn } from "@/lib/cn";

const workspaces = [
  "/home/cloud/projects/kuku",
  "/home/cloud/projects/work",
  "/home/cloud/projects/oss",
];

export function WorkspaceZone() {
  const active = useUIStore((s) => s.workspace);
  const setWorkspace = useUIStore((s) => s.setWorkspace);

  const folderName = (path: string) => path.split("/").pop() ?? path;

  return (
    <div className="h-full flex flex-col p-3 gap-2 overflow-auto">
      <p className="text-[var(--text-xs)] font-medium text-[var(--color-text-muted)] uppercase tracking-wider shrink-0">
        Workspace
      </p>
      <div className="flex-1 flex flex-col gap-1.5">
        {workspaces.map((ws) => (
          <button
            key={ws}
            onClick={() => { setWorkspace(ws); }}
            className={cn(
              "text-left px-3 py-1.5 text-[var(--text-sm)] rounded-[var(--radius-md)] transition-colors cursor-pointer truncate",
              ws === active
                ? "bg-[var(--color-accent)] text-white font-medium"
                : "text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]",
            )}
            title={ws}
          >
            {folderName(ws)}
          </button>
        ))}
      </div>
      <div className="flex gap-1 shrink-0">
        <button
          className="text-[var(--text-xs)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] p-1 rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] transition-colors cursor-pointer"
          aria-label="Add workspace"
          title="Add workspace"
        >
          +
        </button>
        <button
          className="text-[var(--text-xs)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] p-1 rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] transition-colors cursor-pointer"
          aria-label="Workspace settings"
          title="Settings"
        >
          &#x2699;
        </button>
      </div>
    </div>
  );
}
