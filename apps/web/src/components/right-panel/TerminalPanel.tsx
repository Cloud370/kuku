import { cn } from "@/lib/cn";

const exampleOutput = [
  { stream: "stdout" as const, text: "$ cargo build --release" },
  { stream: "stdout" as const, text: "   Compiling kuku v0.1.0" },
  { stream: "stdout" as const, text: "   Compiling kuku-cli v0.1.0" },
  { stream: "stderr" as const, text: "warning: unused import: `std::fmt`" },
  { stream: "stderr" as const, text: " --> src/main.rs:3:5" },
  { stream: "stdout" as const, text: "   Compiling kuku-server v0.1.0" },
  { stream: "stdout" as const, text: "    Finished release [optimized] in 12.34s" },
];

export type TerminalPanelProps = {
  lines?: { stream: "stdout" | "stderr"; text: string }[];
};

export function TerminalPanel({ lines = exampleOutput }: TerminalPanelProps) {
  return (
    <div className="h-full flex flex-col bg-[var(--color-surface)]">
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-[var(--color-border)] shrink-0">
        <span className="text-[var(--text-xs)] text-[var(--color-text-muted)] font-mono">Terminal</span>
        <button className="text-[var(--text-xs)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] cursor-pointer transition-colors">
          Clear
        </button>
      </div>
      <div className="flex-1 overflow-auto p-3 font-mono text-[var(--text-xs)] leading-relaxed">
        {lines.map((line, i) => (
          <div
            key={i}
            className={cn(
              "whitespace-pre-wrap",
              line.stream === "stderr" ? "text-red-400" : "text-green-400/80",
            )}
          >
            {line.text}
          </div>
        ))}
      </div>
    </div>
  );
}
