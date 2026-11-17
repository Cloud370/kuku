import { useMemo } from "react";
import { marked, Renderer } from "marked";
import type { Tokens } from "marked";

const entities: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

function escapeHtml(text: string) {
  return text.replace(/[&<>"']/g, (character) => entities[character] ?? character);
}

class ConversationRenderer extends Renderer {
  override html({ text }: Tokens.HTML | Tokens.Tag) {
    return escapeHtml(text);
  }

  override link({ href, tokens }: Tokens.Link) {
    const text = this.parser.parseInline(tokens);
    const url = URL.parse(href, window.location.href);
    if (!url || !["http:", "https:", "mailto:"].includes(url.protocol)) return text;
    return `<a href="${escapeHtml(url.href)}" target="_blank" rel="noopener noreferrer">${text}</a>`;
  }

  override image({ text }: Tokens.Image) {
    return escapeHtml(text);
  }
}

export type TextContentProps = { text: string };

export function TextContent({ text }: TextContentProps) {
  const html = useMemo(
    () => marked.parse(text, { renderer: new ConversationRenderer(), async: false }),
    [text],
  );

  return (
    <div
      className="prose break-words text-[var(--text-sm)] text-[var(--color-text-primary)] leading-relaxed"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
