import { useEffect } from "react";
import { Routes, Route } from "react-router-dom";
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

function App() {
  return (
    <ThemeProvider>
      <ConnectionGate>
        <Routes>
          <Route path="/" element={<Home />} />
          <Route path="/session/:id" element={<Session />} />
        </Routes>
      </ConnectionGate>
    </ThemeProvider>
  );
}

export default App;
