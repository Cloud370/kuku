import { Panel, Group, Separator } from "react-resizable-panels";
import { useUIStore } from "@/stores/ui";

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
          <div className="h-full flex flex-col p-3 gap-2 overflow-auto">
            <p className="text-[var(--text-xs)] font-medium text-[var(--color-text-muted)] uppercase tracking-wider shrink-0">
              Workspace
            </p>
            <div className="flex-1 flex flex-col gap-1.5">
              <div className="text-[var(--text-sm)] text-[var(--color-text-secondary)]">personal</div>
              <div className="text-[var(--text-sm)] text-[var(--color-text-muted)]">work</div>
              <div className="text-[var(--text-sm)] text-[var(--color-text-muted)]">oss</div>
            </div>
            <div className="flex gap-1 shrink-0">
              <button
                className="text-[var(--text-xs)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] p-1 rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] transition-colors cursor-pointer"
                aria-label="Add workspace"
              >
                +
              </button>
            </div>
          </div>
        </Panel>
        <Separator className={vertSep}>
          <div className={vertLine} />
        </Separator>
        <Panel defaultSize={67} minSize="120px">
          <div className="h-full flex flex-col p-3 gap-2 overflow-auto">
            <p className="text-[var(--text-xs)] font-medium text-[var(--color-text-muted)] uppercase tracking-wider shrink-0">
              Sessions
            </p>
            <div className="flex-1 text-[var(--text-sm)] text-[var(--color-text-muted)]">
              {/* SessionList filled in Task 4 */}
            </div>
          </div>
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
