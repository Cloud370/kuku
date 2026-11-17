import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => {
  sessionStorage.clear();
  window.history.replaceState(null, "", "/");
  vi.unstubAllGlobals();
});

describe("same-origin browser handoff", () => {
  it("removes the one-time ticket before exchange and reuses the scoped credential on reload", async () => {
    vi.resetModules();
    window.history.replaceState(
      null,
      "",
      "/?workspace=%2Fwork&session=s1&agent=main#ticket=one-time",
    );
    const capture = vi.fn((path: string, init?: RequestInit) => {
      expect(location.hash).toBe("");
      const headers = new Headers(init?.headers);
      if (path === "/service/browser-session") {
        expect(init?.body).toBe(JSON.stringify({ ticket: "one-time" }));
        expect(headers.get("Authorization")).toBeNull();
        return Promise.resolve(Response.json({ token: "browser-only-token" }));
      }
      expect(headers.get("Authorization")).toBe("Bearer browser-only-token");
      return Promise.resolve(Response.json({ ok: true }));
    });
    vi.stubGlobal("fetch", capture);
    const api = await import("./client");
    await Promise.all([api.exchangeBrowserTicket(), api.exchangeBrowserTicket()]);
    expect(capture).toHaveBeenCalledTimes(1);
    expect(location.search).toBe("?workspace=%2Fwork&session=s1&agent=main");
    expect(location.href).not.toContain("browser-only-token");
    vi.resetModules();
    const reloaded = await import("./client");
    await reloaded.get("/service/status");
    expect(capture).toHaveBeenCalledTimes(2);
  });
});
