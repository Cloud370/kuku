import type { ReactNode } from "react";
import { useUIStore } from "@/stores/ui";
import type { RightPanelTab } from "@/stores/ui";
import { TabBar } from "./TabBar";

export type RightPanelProps = {
  diffContent?: ReactNode;
  terminalContent?: ReactNode;
  statusContent?: ReactNode;
};

export function RightPanel({ diffContent, terminalContent, statusContent }: RightPanelProps) {
  const tab = useUIStore((s) => s.rightPanelTab);
  const setTab = useUIStore((s) => s.setRightPanelTab);

  return (
    <div className="h-full flex flex-col bg-[var(--color-surface)]">
      <TabBar active={tab} onChange={(t: RightPanelTab) => { setTab(t); }} />
      <div className="flex-1 min-h-0 overflow-auto">
        {tab === "diff" && (diffContent ?? <EmptyPlaceholder label="Diff" />)}
        {tab === "terminal" && (terminalContent ?? <EmptyPlaceholder label="Terminal" />)}
        {tab === "status" && (statusContent ?? <EmptyPlaceholder label="Status" />)}
      </div>
    </div>
  );
}

function EmptyPlaceholder({ label }: { label: string }) {
  return (
    <div className="flex items-center justify-center h-full text-[var(--text-sm)] text-[var(--color-text-muted)]">
      {label} panel
    </div>
  );
}
