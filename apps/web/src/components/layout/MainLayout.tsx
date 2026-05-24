import { Panel, Group, Separator } from "react-resizable-panels";
import { useUIStore } from "@/stores/ui";
import { Header } from "./Header";
import { LeftSidebar } from "./LeftSidebar";

const separatorClasses =
  "w-[6px] flex items-center justify-center group data-[resize-handle-active]:bg-[var(--color-accent-muted)] transition-colors";
const separatorLineClasses =
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
  const setLeftSidebarWidth = useUIStore((s) => s.setLeftSidebarWidth);
  const setRightPanelWidth = useUIStore((s) => s.setRightPanelWidth);
  const leftSidebarWidth = useUIStore((s) => s.leftSidebarWidth);
  const rightPanelWidth = useUIStore((s) => s.rightPanelWidth);

  const showLeft = layoutMode === "two-column" || layoutMode === "three-column";
  const showRight = layoutMode === "three-column";

  const toPct = (px: number): number => Math.round((px / 1200) * 100);

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
          {showLeft && (
            <>
              <Panel
                defaultSize={toPct(leftSidebarWidth)}
                minSize={12}
                maxSize={30}
                collapsible
                collapsedSize={4}
                onResize={(ps) => setLeftSidebarWidth(ps.inPixels)}
              >
                <LeftSidebar />
              </Panel>
              <Separator className={separatorClasses}>
                <div className={separatorLineClasses} />
              </Separator>
            </>
          )}
          <Panel minSize={showRight ? 25 : 40}>
            <main className="h-full flex flex-col min-h-0">{children}</main>
          </Panel>
          {showRight && (
            <>
              <Separator className={separatorClasses}>
                <div className={separatorLineClasses} />
              </Separator>
              <Panel
                defaultSize={toPct(rightPanelWidth)}
                minSize={15}
                maxSize={40}
                collapsible
                collapsedSize={4}
                onResize={(ps) => setRightPanelWidth(ps.inPixels)}
              >
                <aside className="h-full border-l border-[var(--color-border)] bg-[var(--color-surface)]">
                  {rightPanel ?? (
                    <div className="p-4 text-[var(--text-sm)] text-[var(--color-text-muted)]">
                      Right Panel
                    </div>
                  )}
                </aside>
              </Panel>
            </>
          )}
        </Group>
      </div>
    </div>
  );
}
