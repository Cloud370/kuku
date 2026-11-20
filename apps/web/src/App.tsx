import { useEffect } from "react";
import { Routes, Route, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { errorMessage } from "@/api/client";
import { useCommands } from "@/stores/commands";
import { targetKey } from "@/stores/drafts";
import { useUIStore } from "@/stores/ui";
import { useWorkspaces, useWorkspaceResolution } from "@/queries/sessions";
import { ConnectionGate } from "@/components/ConnectionGate";
import { CommandRecovery } from "@/components/CommandRecovery";
import { WorkspaceZone } from "@/components/layout/WorkspaceZone";
import { Home } from "@/routes/Home";
import { Session } from "@/routes/Session";
import { workspaceHref } from "@/routes/navigation";

function Browser() {
  const { id } = useParams<{ id: string }>();
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const lastWorkspace = useUIStore((s) => s.workspace);
  const setWorkspace = useUIStore((s) => s.setWorkspace);
  const dismissOutcome = useCommands((s) => s.dismissOutcome);
  const workspaces = useWorkspaces();
  const requested =
    params.get("workspace") ?? (lastWorkspace || workspaces.data?.workspaces[0] || "");
  const resolved = useWorkspaceResolution(requested);
  const workspace = resolved.data?.workspace ?? "";

  useEffect(() => {
    if (workspace && workspace !== lastWorkspace) setWorkspace(workspace);
  }, [lastWorkspace, setWorkspace, workspace]);

  const workspacePanel = (
    <WorkspaceZone
      workspace={workspace || requested}
      workspaces={workspaces.data?.workspaces}
      error={
        resolved.error
          ? errorMessage(resolved.error)
          : workspaces.error
            ? errorMessage(workspaces.error)
            : undefined
      }
      onSelect={(path) => {
        dismissOutcome();
        void navigate(workspaceHref(path));
      }}
    />
  );
  const existing = params.get("session") ?? (id === "new" ? undefined : id);
  const agent = params.get("agent") || "main";
  if (workspace && (existing || id === "new")) {
    const target = { workspace, session: existing ?? "", agent };
    return <Session key={targetKey(target)} target={target} workspacePanel={workspacePanel} />;
  }
  return <Home workspace={workspace} workspacePanel={workspacePanel} />;
}

function App() {
  const theme = useUIStore((s) => s.theme);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  return (
    <ConnectionGate>
      <div className="flex flex-col h-screen overflow-hidden bg-[var(--color-surface)]">
        <CommandRecovery />
        <div className="flex-1 min-h-0">
          <Routes>
            <Route path="/" element={<Browser />} />
            <Route path="/session/:id" element={<Browser />} />
          </Routes>
        </div>
      </div>
    </ConnectionGate>
  );
}

export default App;
