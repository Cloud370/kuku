import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { TextContent } from "./TextContent";

describe("conversation Markdown", () => {
  it("formats text while rendering raw HTML and unsafe URLs as text", () => {
    const { container } = render(
      <TextContent
        text={[
          "**Code** and [safe docs](https://example.org/docs)",
          "",
          "[blocked](javascript:void(0)) ![alt text](https://example.org/pixel)",
          "",
          "<script>ignored</script>",
        ].join("\n")}
      />,
    );
    expect(container.querySelector("strong")?.textContent).toBe("Code");
    expect(container.querySelectorAll("a")).toHaveLength(1);
    expect(container.querySelector("a")?.getAttribute("rel")).toBe("noopener noreferrer");
    expect(container.querySelector("script, img")).toBeNull();
    expect(container.textContent).toContain("<script>ignored</script>");
  });
});
