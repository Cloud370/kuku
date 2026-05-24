import { cn } from "@/lib/cn";
import type { RightPanelTab } from "@/stores/ui";

const tabs: { id: RightPanelTab; label: string }[] = [
  { id: "diff", label: "Diff" },
  { id: "terminal", label: "Terminal" },
  { id: "status", label: "Status" },
];

export type TabBarProps = {
  active: RightPanelTab;
  onChange: (tab: RightPanelTab) => void;
};

export function TabBar({ active, onChange }: TabBarProps) {
  return (
    <div className="flex border-b border-[var(--color-border)] shrink-0">
      {tabs.map((tab) => (
        <button
          key={tab.id}
          onClick={() => onChange(tab.id)}
          className={cn(
            "px-3 py-2 text-[var(--text-xs)] font-medium transition-colors cursor-pointer border-b-2 -mb-[1px]",
            tab.id === active
              ? "text-[var(--color-accent)] border-[var(--color-accent)]"
              : "text-[var(--color-text-muted)] border-transparent hover:text-[var(--color-text-secondary)]",
          )}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );
}
