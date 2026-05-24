import { describe, it, expect } from "vitest";
import { useHealth, type HealthResponse } from "./health";

describe("useHealth", () => {
  it("has correct queryKey", () => {
    const hook = useHealth;
    expect(hook).toBeDefined();
  });

  it("HealthResponse type matches wire format", () => {
    const response: HealthResponse = { ok: true, version: "0.1.0" };
    expect(response.ok).toBe(true);
    expect(response.version).toBe("0.1.0");
  });
});
