import { Panel, Group, Separator } from "react-resizable-panels";
import { useUIStore } from "@/stores/ui";
import { WorkspaceZone } from "./WorkspaceZone";
import { SessionList } from "./SessionList";

const vertSep =
  "group flex items-center justify-center h-[6px] shrink-0 hover:bg-[var(--color-accent-muted)] transition-colors";
const vertLine =
  "h-[1px] w-1/3 bg-[var(--color-border)] group-hover:bg-[var(--color-accent)] transition-colors";

export function LeftSidebar() {
  const theme = useUIStore((s) => s.theme);
  const toggleTheme = useUIStore((s) => s.toggleTheme);

  return (
    <aside className="h-full flex flex-col bg-[var(--color-surface)] border-r border-[var(--color-border)]">
      <Group orientation="vertical" className="flex-1 min-h-0">
        <Panel defaultSize={33} minSize="80px" maxSize="50%">
          <WorkspaceZone />
        </Panel>
        <Separator className={vertSep}>
          <div className={vertLine} />
        </Separator>
        <Panel defaultSize={67} minSize="120px">
          <SessionList />
        </Panel>
      </Group>
      <div className="border-t border-[var(--color-border)] p-2 flex justify-center shrink-0">
        <button
          onClick={toggleTheme}
          className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] text-[var(--text-lg)] cursor-pointer select-none transition-colors p-1 rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)]"
          aria-label="Toggle theme"
        >
          {theme === "dark" ? "☀" : "☽"}
        </button>
      </div>
    </aside>
  );
}
