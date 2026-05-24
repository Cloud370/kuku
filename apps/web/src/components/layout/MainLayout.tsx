import { Panel, Group, Separator } from "react-resizable-panels";
import { useUIStore } from "@/stores/ui";
import { Header } from "./Header";
import { LeftSidebar } from "./LeftSidebar";

const horizSep =
  "group flex items-center justify-center w-[6px] shrink-0 hover:bg-[var(--color-accent-muted)] transition-colors";
const horizLine =
  "w-[1px] h-1/3 bg-[var(--color-border)] group-hover:bg-[var(--color-accent)] transition-colors";

export type MainLayoutProps = {
  sessionTitle?: string;
  sessionStatus?: "running" | "waiting" | "completed" | "idle";
  children?: React.ReactNode;
  rightPanel?: React.ReactNode;
};

export function MainLayout({
  sessionTitle,
  sessionStatus,
  children,
  rightPanel,
}: MainLayoutProps) {
  const layoutMode = useUIStore((s) => s.layoutMode);
  const setLayoutMode = useUIStore((s) => s.setLayoutMode);

  const showLeft = layoutMode === "two-column" || layoutMode === "three-column";
  const showRight = layoutMode === "three-column";

  return (
    <div className="h-screen flex flex-col overflow-hidden bg-[var(--color-surface)]">
      <Header
        sessionTitle={sessionTitle}
        sessionStatus={sessionStatus}
        layoutMode={layoutMode}
        onLayoutModeChange={setLayoutMode}
      />
      <div className="flex-1 min-h-0">
        <Group orientation="horizontal">
          {showLeft ? (
            <>
              <Panel defaultSize={22} minSize="180px" maxSize="400px">
                <LeftSidebar />
              </Panel>
              <Separator className={horizSep}>
                <div className={horizLine} />
              </Separator>
            </>
          ) : (
            <Panel defaultSize={0} minSize={0} maxSize={0} />
          )}
          <Panel defaultSize={showRight ? 53 : 78} minSize="300px">
            <main className="h-full flex flex-col min-h-0">{children}</main>
          </Panel>
          {showRight ? (
            <>
              <Separator className={horizSep}>
                <div className={horizLine} />
              </Separator>
              <Panel defaultSize={25} minSize="280px" maxSize="40vw">
                <aside className="h-full border-l border-[var(--color-border)] bg-[var(--color-surface)]">
                  {rightPanel ?? (
                    <div className="p-4 text-[var(--text-sm)] text-[var(--color-text-muted)]">
                      Right Panel
                    </div>
                  )}
                </aside>
              </Panel>
            </>
          ) : (
            <Panel defaultSize={0} minSize={0} maxSize={0} />
          )}
        </Group>
      </div>
    </div>
  );
}
