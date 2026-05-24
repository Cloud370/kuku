const exampleContext = {
  model: "claude-opus-4-7",
  provider: "anthropic",
  turn: 3,
  tokens: { input: 12450, output: 3201 },
  sessionId: "sess-a3f2",
  workspace: "/home/cloud/projects/kuku",
};

const exampleJsonl = [
  '{"type":"text","turn":3,"content":"Let me check the API configuration..."}',
  '{"type":"tool_start","turn":3,"tool":"read_file","args":{"path":"src/api/gateway.ts"}}',
  '{"type":"tool_output","turn":3,"tool":"read_file","content":"..."}',
  '{"type":"text","turn":3,"content":"The configuration looks correct."}',
];

export type StatusPanelProps = {
  context?: typeof exampleContext;
  events?: string[];
};

export function StatusPanel({ context = exampleContext, events = exampleJsonl }: StatusPanelProps) {
  return (
    <div className="h-full flex flex-col">
      <div className="p-3 border-b border-[var(--color-border)] shrink-0 space-y-1.5 text-[var(--text-xs)]">
        <div className="flex justify-between">
          <span className="text-[var(--color-text-muted)]">Model</span>
          <span className="text-[var(--color-text-primary)] font-mono">{context.model}</span>
        </div>
        <div className="flex justify-between">
          <span className="text-[var(--color-text-muted)]">Provider</span>
          <span className="text-[var(--color-text-primary)]">{context.provider}</span>
        </div>
        <div className="flex justify-between">
          <span className="text-[var(--color-text-muted)]">Turn</span>
          <span className="text-[var(--color-text-primary)]">{context.turn}</span>
        </div>
        <div className="flex justify-between">
          <span className="text-[var(--color-text-muted)]">Tokens</span>
          <span className="text-[var(--color-text-primary)] font-mono">
            {context.tokens.input} in / {context.tokens.output} out
          </span>
        </div>
        <div className="flex justify-between">
          <span className="text-[var(--color-text-muted)]">Session</span>
          <span className="text-[var(--color-text-primary)] font-mono text-[var(--text-xs)]">
            {context.sessionId}
          </span>
        </div>
        <div className="flex justify-between">
          <span className="text-[var(--color-text-muted)]">Workspace</span>
          <span className="text-[var(--color-text-primary)] font-mono text-[var(--text-xs)] truncate ml-4">
            {context.workspace}
          </span>
        </div>
      </div>
      <div className="flex-1 min-h-0 flex flex-col">
        <p className="text-[var(--text-xs)] font-medium text-[var(--color-text-muted)] px-3 py-1.5 border-b border-[var(--color-border)] shrink-0">
          Event Log
        </p>
        <div className="flex-1 overflow-auto p-3 font-mono text-[var(--text-xs)] leading-relaxed space-y-0.5">
          {events.map((line, i) => (
            <div key={i} className="text-[var(--color-text-muted)] whitespace-pre-wrap break-all">
              {line}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
