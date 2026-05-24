import { useMemo } from "react";
import { marked } from "marked";

export type TextContentProps = {
  text: string;
};

export function TextContent({ text }: TextContentProps) {
  const html = useMemo(() => marked.parse(text) as string, [text]);

  return (
    <div
      className="prose text-[var(--text-sm)] text-[var(--color-text-primary)] leading-relaxed"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
