interface StatusContext {
  model?: string;
  provider?: string;
  turn?: number | null;
  tokens?: { input_tokens: number | null; output_tokens: number | null };
  sessionId?: string;
  workspace?: string;
  agent?: string;
}

const exampleContext: StatusContext = {
  model: "claude-opus-4-7",
  provider: "anthropic",
  turn: 3,
  tokens: { input_tokens: 12450, output_tokens: 3201 },
  sessionId: "sess-a3f2",
  workspace: "/home/cloud/projects/kuku",
};

const exampleEvents = ["#6 user", "#9 assistant", "#10 tool_call", "#11 tool_result"];

export type StatusPanelProps = {
  context?: StatusContext;
  events?: string[];
};

export function StatusPanel({
  context = exampleContext,
  events = exampleEvents,
}: StatusPanelProps) {
  const rows = [
    ["Workspace", context.workspace],
    ["Session", context.sessionId],
    ["Agent", context.agent],
    ["Model", context.model],
    ["Connection", context.provider],
    ["Turn", context.turn],
    [
      "Tokens",
      context.tokens &&
        `${String(context.tokens.input_tokens ?? "—")} in / ${String(context.tokens.output_tokens ?? "—")} out`,
    ],
  ];

  return (
    <div className="h-full flex flex-col">
      <dl className="p-3 border-b border-[var(--color-border)] shrink-0 space-y-1.5 text-[var(--text-xs)]">
        {rows.map(
          ([name, value]) =>
            value !== undefined &&
            value !== null &&
            value !== "" && (
              <div className="flex justify-between gap-4" key={name}>
                <dt className="text-[var(--color-text-muted)] shrink-0">{name}</dt>
                <dd
                  className="truncate text-[var(--color-text-primary)] font-mono"
                  title={String(value)}
                >
                  {value}
                </dd>
              </div>
            ),
        )}
      </dl>
      {events.length > 0 && (
        <div className="flex-1 min-h-0 flex flex-col">
          <p className="text-[var(--text-xs)] font-medium text-[var(--color-text-muted)] px-3 py-1.5 border-b border-[var(--color-border)]">
            Event Log
          </p>
          <div className="flex-1 overflow-auto p-3 font-mono text-[var(--text-xs)] leading-relaxed space-y-0.5">
            {events.map((line, index) => (
              <p
                key={index}
                className="text-[var(--color-text-muted)] whitespace-pre-wrap break-all"
              >
                {line}
              </p>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
