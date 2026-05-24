import { useState } from "react";
import { cn } from "@/lib/cn";

interface FileEntry {
  name: string;
  lines: number;
  snippet: string;
}

const exampleFiles: FileEntry[] = [
  { name: "src/api/gateway.ts", lines: 142, snippet: 'export class Gateway {\n  private rateLimiter: RateLimiter;\n  constructor(config: GatewayConfig) {\n    this.rateLimiter = new RateLimiter(config.limit);' },
  { name: "Dockerfile", lines: 48, snippet: 'FROM rust:1.85-slim AS builder\nWORKDIR /app\nCOPY Cargo.toml Cargo.lock ./\nRUN cargo build --release' },
  { name: "config/default.toml", lines: 23, snippet: '[gateway]\nport = 8080\nrate_limit = 1000\nbackend_url = "http://localhost:3001"' },
];

export type FileAccessBarProps = {
  files?: FileEntry[];
};

export function FileAccessBar({ files = exampleFiles }: FileAccessBarProps) {
  const [open, setOpen] = useState(false);

  return (
    <div className="mb-2 rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface)] overflow-hidden">
      <button
        onClick={() => setOpen(!open)}
        className="w-full flex items-center gap-2 px-3 py-2 text-[var(--text-xs)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors cursor-pointer"
      >
        <span className="font-mono text-[var(--text-sm)]">&#x1F4C4;</span>
        <span>
          Read {files.length} file{files.length !== 1 ? "s" : ""}
        </span>
        <span className={cn("ml-auto text-[var(--text-xs)] transition-transform", open && "rotate-180")}>
          &#x25BC;
        </span>
      </button>
      {open && (
        <div className="border-t border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {files.map((f) => (
            <div key={f.name} className="px-3 py-2">
              <div className="flex items-center gap-2 mb-1">
                <span className="font-mono text-[var(--text-xs)] text-[var(--color-text-primary)]">
                  {f.name}
                </span>
                <span className="font-mono text-[var(--text-xs)] text-[var(--color-text-muted)]">
                  {f.lines} lines
                </span>
              </div>
              <pre className="font-mono text-[var(--text-xs)] text-[var(--color-text-secondary)] bg-[var(--color-surface)] rounded-[var(--radius-sm)] p-2 overflow-x-auto border border-[var(--color-border)]">
                {f.snippet}
              </pre>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
