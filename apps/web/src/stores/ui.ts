import { create } from "zustand";
import { persist } from "zustand/middleware";

export type LayoutMode = "one-column" | "two-column" | "three-column";
export type RightPanelTab = "diff" | "terminal" | "status";

interface UIState {
  theme: "dark" | "light";
  workspace: string;
  setTheme: (theme: "dark" | "light") => void;
  setWorkspace: (workspace: string) => void;
  toggleTheme: () => void;

  leftSidebarOpen: boolean;
  leftSidebarWidth: number;
  rightPanelOpen: boolean;
  rightPanelWidth: number;
  rightPanelTab: RightPanelTab;
  layoutMode: LayoutMode;
  workspaceZoneSize: number;

  setLeftSidebarOpen: (open: boolean) => void;
  toggleLeftSidebar: () => void;
  setLeftSidebarWidth: (width: number) => void;
  setRightPanelOpen: (open: boolean) => void;
  toggleRightPanel: () => void;
  setRightPanelWidth: (width: number) => void;
  setRightPanelTab: (tab: RightPanelTab) => void;
  setLayoutMode: (mode: LayoutMode) => void;
  setWorkspaceZoneSize: (size: number) => void;
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

      leftSidebarOpen: true,
      leftSidebarWidth: 260,
      rightPanelOpen: true,
      rightPanelWidth: 400,
      rightPanelTab: "diff",
      layoutMode: "three-column",
      workspaceZoneSize: 33,

      setLeftSidebarOpen: (open) => set({ leftSidebarOpen: open }),
      toggleLeftSidebar: () => set({ leftSidebarOpen: !get().leftSidebarOpen }),
      setLeftSidebarWidth: (width) => set({ leftSidebarWidth: width }),
      setRightPanelOpen: (open) => set({ rightPanelOpen: open }),
      toggleRightPanel: () => set({ rightPanelOpen: !get().rightPanelOpen }),
      setRightPanelWidth: (width) => set({ rightPanelWidth: width }),
      setRightPanelTab: (tab) => set({ rightPanelTab: tab }),
      setLayoutMode: (mode) => set({ layoutMode: mode }),
      setWorkspaceZoneSize: (size) => set({ workspaceZoneSize: size }),
    }),
    {
      name: "kuku-ui",
      partialize: (state) => ({
        theme: state.theme,
        workspace: state.workspace,
        leftSidebarWidth: state.leftSidebarWidth,
        rightPanelWidth: state.rightPanelWidth,
        rightPanelTab: state.rightPanelTab,
      }),
    },
  ),
);
