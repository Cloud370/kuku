import { useState } from "react";
import { cn } from "@/lib/cn";

interface DiffLine {
  type: "added" | "removed" | "unchanged";
  content: string;
  oldLineNum?: number;
  newLineNum?: number;
}

const exampleDiff: DiffLine[] = [
  { type: "unchanged", content: "use std::collections::HashMap;", oldLineNum: 1, newLineNum: 1 },
  { type: "unchanged", content: "", oldLineNum: 2, newLineNum: 2 },
  { type: "removed", content: "fn old_handler(req: Request) -> Response {", oldLineNum: 3 },
  { type: "removed", content: "    let body = req.body();", oldLineNum: 4 },
  { type: "removed", content: "    process(body)", oldLineNum: 5 },
  { type: "added", content: "async fn new_handler(req: Request) -> Result<Response> {", newLineNum: 3 },
  { type: "added", content: "    let body = req.json::<Payload>()?;", newLineNum: 4 },
  { type: "added", content: "    process_async(body).await", newLineNum: 5 },
  { type: "unchanged", content: "}", oldLineNum: 6, newLineNum: 6 },
  { type: "unchanged", content: "", oldLineNum: 7, newLineNum: 7 },
  { type: "unchanged", content: "fn main() {", oldLineNum: 8, newLineNum: 8 },
  { type: "removed", content: "    old_handler(Request::default());", oldLineNum: 9 },
  { type: "added", content: "    tokio::spawn(new_handler(Request::default()));", newLineNum: 9 },
  { type: "unchanged", content: "}", oldLineNum: 10, newLineNum: 10 },
];

const lineBg: Record<DiffLine["type"], string> = {
  added: "bg-green-950/30",
  removed: "bg-red-950/30",
  unchanged: "",
};

const lineSign: Record<DiffLine["type"], string> = {
  added: "+",
  removed: "−",
  unchanged: "",
};

export type DiffViewerProps = {
  lines?: DiffLine[];
};

export function DiffViewer({ lines = exampleDiff }: DiffViewerProps) {
  const [mode, setMode] = useState<"unified" | "split">("unified");

  return (
    <div className="h-full flex flex-col">
      <div className="flex items-center gap-2 px-3 py-1.5 border-b border-[var(--color-border)] shrink-0">
        <button
          onClick={() => setMode("unified")}
          className={cn(
            "text-[var(--text-xs)] px-2 py-0.5 rounded-[var(--radius-sm)] cursor-pointer transition-colors",
            mode === "unified"
              ? "bg-[var(--color-accent-muted)] text-[var(--color-accent)]"
              : "text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
          )}
        >
          Unified
        </button>
        <button
          onClick={() => setMode("split")}
          className={cn(
            "text-[var(--text-xs)] px-2 py-0.5 rounded-[var(--radius-sm)] cursor-pointer transition-colors",
            mode === "split"
              ? "bg-[var(--color-accent-muted)] text-[var(--color-accent)]"
              : "text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
          )}
        >
          Split
        </button>
      </div>
      <div className="flex-1 overflow-auto font-mono text-[var(--text-xs)]">
        {mode === "unified" ? (
          <table className="w-full">
            <tbody>
              {lines.map((line, i) => (
                <tr key={i} className={lineBg[line.type]}>
                  <td className="text-right pr-2 pl-2 text-[var(--color-text-muted)] select-none w-[3em]">
                    {line.oldLineNum}
                  </td>
                  <td className="text-right pr-2 text-[var(--color-text-muted)] select-none w-[3em]">
                    {line.newLineNum}
                  </td>
                  <td className="pr-2 text-[var(--color-text-muted)] select-none w-[1.5em] text-center">
                    {lineSign[line.type]}
                  </td>
                  <td className="pr-4 text-[var(--color-text-primary)] whitespace-pre">
                    {line.content}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <div className="grid grid-cols-2">
            <div className="border-r border-[var(--color-border)]">
              {lines.map((line, i) => (
                <div key={`old-${i}`} className={cn("flex", line.type === "removed" ? "bg-red-950/30" : "")}>
                  <span className="text-right w-[3em] pr-2 text-[var(--color-text-muted)] select-none shrink-0">
                    {line.oldLineNum}
                  </span>
                  <span className="text-[var(--color-text-primary)] whitespace-pre">
                    {line.type !== "added" ? line.content : ""}
                  </span>
                </div>
              ))}
            </div>
            <div>
              {lines.map((line, i) => (
                <div key={`new-${i}`} className={cn("flex", line.type === "added" ? "bg-green-950/30" : "")}>
                  <span className="text-right w-[3em] pr-2 text-[var(--color-text-muted)] select-none shrink-0">
                    {line.newLineNum}
                  </span>
                  <span className="text-[var(--color-text-primary)] whitespace-pre">
                    {line.type !== "removed" ? line.content : ""}
                  </span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
