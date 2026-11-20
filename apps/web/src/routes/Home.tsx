import type { ReactNode } from "react";
import { Header } from "@/components/layout/Header";
import { SessionBrowser } from "@/components/layout/SessionBrowser";
import { useUIStore } from "@/stores/ui";

export function Home({
  workspace,
  workspacePanel,
}: {
  workspace: string;
  workspacePanel: ReactNode;
}) {
  const layoutMode = useUIStore((s) => s.layoutMode);
  const setLayoutMode = useUIStore((s) => s.setLayoutMode);

  return (
    <div className="h-full flex flex-col bg-[var(--color-surface)]">
      <Header sessionTitle="kuku" layoutMode={layoutMode} onLayoutModeChange={setLayoutMode} />
      <div className="flex-1 min-h-0 flex flex-col max-w-2xl mx-auto w-full">
        <div className="max-h-48 overflow-auto">{workspacePanel}</div>
        <div className="min-h-0 flex-1">
          <SessionBrowser workspace={workspace} />
        </div>
      </div>
    </div>
  );
}
