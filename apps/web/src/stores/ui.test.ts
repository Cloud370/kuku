import { describe, it, expect, beforeEach } from "vitest";
import { useUIStore } from "./ui";

describe("useUIStore", () => {
  beforeEach(() => {
    useUIStore.setState({ theme: "dark", workspace: "personal" });
    document.documentElement.dataset.theme = "dark";
  });

  it("has dark theme by default", () => {
    expect(useUIStore.getState().theme).toBe("dark");
  });

  it("toggleTheme switches dark→light→dark", () => {
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
});
