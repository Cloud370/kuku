import { useState, useEffect } from "react";
import { useUIStore } from "@/stores/ui";
import { ConnectionGate } from "@/components/ConnectionGate";
import { Home } from "@/routes/Home";
import { Session } from "@/routes/Session";

function ThemeProvider({ children }: { children: React.ReactNode }) {
  const theme = useUIStore((s) => s.theme);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  return <>{children}</>;
}

function Header() {
  const workspace = useUIStore((s) => s.workspace);
  const setWorkspace = useUIStore((s) => s.setWorkspace);
  const toggleTheme = useUIStore((s) => s.toggleTheme);
  const theme = useUIStore((s) => s.theme);

  const workspaces = ["personal", "work", "oss"];

  return (
    <header className="flex items-center justify-between h-[44px] px-4 border-b border-[var(--color-border)] bg-[var(--color-surface-raised)]">
      <nav className="flex gap-1">
        {workspaces.map((ws) => (
          <button
            key={ws}
            onClick={() => { setWorkspace(ws); }}
            data-active={ws === workspace}
            className="px-3 py-1 text-[var(--text-xs)] rounded-[var(--radius-full)]
              data-[active=true]:bg-[var(--color-accent)] data-[active=true]:text-white
              data-[active=false]:text-[var(--color-text-secondary)]
              data-[active=false]:hover:bg-[var(--color-surface-hover)]
              transition-colors cursor-pointer"
          >
            {ws}
          </button>
        ))}
      </nav>
      <button
        onClick={toggleTheme}
        className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] text-[var(--text-lg)] cursor-pointer select-none transition-colors"
        aria-label="Toggle theme"
      >
        {theme === "dark" ? "☀" : "☽"}
      </button>
    </header>
  );
}

type Route = ["home"] | ["session", string];

function App() {
  const [route] = useState<Route>(["home"]);

  return (
    <ThemeProvider>
      <ConnectionGate>
        <Header />
        <main>
          {route[0] === "home" ? (
            <Home />
          ) : (
            <Session id={route[1]} />
          )}
        </main>
      </ConnectionGate>
    </ThemeProvider>
  );
}

export default App;
