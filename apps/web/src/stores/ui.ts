import { create } from "zustand";
import { persist } from "zustand/middleware";

interface UIState {
  theme: "dark" | "light";
  workspace: string;
  setTheme: (theme: "dark" | "light") => void;
  setWorkspace: (workspace: string) => void;
  toggleTheme: () => void;
}

const initialTheme = (): "dark" | "light" => {
  const t = document.documentElement.dataset.theme;
  return t === "light" ? "light" : "dark";
};

export const useUIStore = create<UIState>()(
  persist(
    (set, get) => ({
      theme: initialTheme(),
      workspace: "personal",

      setTheme: (theme) => {
        document.documentElement.dataset.theme = theme;
        set({ theme });
      },

      setWorkspace: (workspace) => set({ workspace }),

      toggleTheme: () => {
        const next = get().theme === "dark" ? "light" : "dark";
        document.documentElement.dataset.theme = next;
        set({ theme: next });
      },
    }),
    {
      name: "kuku-ui",
      partialize: (state) => ({ theme: state.theme, workspace: state.workspace }),
    },
  ),
);
