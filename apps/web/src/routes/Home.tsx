import { useNavigate } from "react-router-dom";
import { Header } from "@/components/layout/Header";
import { SessionList } from "@/components/layout/SessionList";
import { useUIStore } from "@/stores/ui";
import type { LayoutMode } from "@/stores/ui";

export function Home() {
  const navigate = useNavigate();
  const layoutMode = useUIStore((s) => s.layoutMode);
  const setLayoutMode = useUIStore((s) => s.setLayoutMode);

  return (
    <div className="h-screen flex flex-col bg-[var(--color-surface)]">
      <Header
        sessionTitle="kuku"
        layoutMode={layoutMode}
        onLayoutModeChange={(m: LayoutMode) => { setLayoutMode(m); }}
      />
      <div className="flex-1 min-h-0 max-w-2xl mx-auto w-full">
        <SessionList onSelect={(id) => { void navigate(`/session/${id}`); }} />
      </div>
    </div>
  );
}
