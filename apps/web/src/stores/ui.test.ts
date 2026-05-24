import { describe, it, expect, beforeEach } from "vitest";
import { useUIStore } from "./ui";

describe("useUIStore", () => {
  beforeEach(() => {
    useUIStore.setState({
      theme: "dark",
      workspace: "personal",
      leftSidebarOpen: true,
      leftSidebarWidth: 260,
      rightPanelOpen: true,
      rightPanelWidth: 400,
      rightPanelTab: "diff",
      layoutMode: "three-column",
    });
    document.documentElement.dataset.theme = "dark";
  });

  it("has dark theme by default", () => {
    expect(useUIStore.getState().theme).toBe("dark");
  });

  it("toggleTheme switches dark->light->dark", () => {
    const store = useUIStore.getState();
    store.toggleTheme();
    expect(useUIStore.getState().theme).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");

    useUIStore.getState().toggleTheme();
    expect(useUIStore.getState().theme).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("setTheme updates DOM data-theme", () => {
    useUIStore.getState().setTheme("light");
    expect(useUIStore.getState().theme).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("setWorkspace updates value", () => {
    useUIStore.getState().setWorkspace("work");
    expect(useUIStore.getState().workspace).toBe("work");
  });

  it("toggleLeftSidebar toggles state", () => {
    useUIStore.getState().toggleLeftSidebar();
    expect(useUIStore.getState().leftSidebarOpen).toBe(false);

    useUIStore.getState().toggleLeftSidebar();
    expect(useUIStore.getState().leftSidebarOpen).toBe(true);
  });

  it("toggleRightPanel toggles state", () => {
    useUIStore.getState().toggleRightPanel();
    expect(useUIStore.getState().rightPanelOpen).toBe(false);

    useUIStore.getState().toggleRightPanel();
    expect(useUIStore.getState().rightPanelOpen).toBe(true);
  });

  it("setLeftSidebarWidth updates width", () => {
    useUIStore.getState().setLeftSidebarWidth(320);
    expect(useUIStore.getState().leftSidebarWidth).toBe(320);
  });

  it("setRightPanelWidth updates width", () => {
    useUIStore.getState().setRightPanelWidth(360);
    expect(useUIStore.getState().rightPanelWidth).toBe(360);
  });

  it("setRightPanelTab updates tab", () => {
    useUIStore.getState().setRightPanelTab("terminal");
    expect(useUIStore.getState().rightPanelTab).toBe("terminal");
  });

  it("setLayoutMode updates mode", () => {
    useUIStore.getState().setLayoutMode("two-column");
    expect(useUIStore.getState().layoutMode).toBe("two-column");
  });
});
