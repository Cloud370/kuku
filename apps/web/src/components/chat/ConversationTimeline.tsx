import type { Entry, Target, ToolCall, ToolResult } from "@/api/sessions";
import type { ConversationView, ToolPreview } from "@/adapters/observation";
import { visibleOutput } from "@/lib/output";
import { ErrorNotice } from "./ErrorNotice";
import { CancelledNotice } from "./CancelledNotice";
import { RetainedOutput } from "./RetainedOutput";
import { TextContent } from "./TextContent";
import { ThinkingBlock } from "./ThinkingBlock";
import { ToolCard } from "./ToolCard";
import { TurnCard } from "./TurnCard";

const callKey = (turn?: number | null, id?: string) => `${String(turn ?? "")}:${id ?? ""}`;

function Tool({
  target,
  call,
  result,
  preview,
  active = true,
}: {
  target: Target;
  call?: ToolCall;
  result?: ToolResult;
  preview?: ToolPreview;
  active?: boolean;
}) {
  const information = result?.structured;
  const diff = typeof information?.diff === "string" ? information.diff : undefined;
  const exitCode = typeof information?.exit_code === "number" ? information.exit_code : undefined;
  const name =
    call?.tool ??
    result?.tool ??
    preview?.name ??
    `Tool ${result?.tool_call_id ?? preview?.id ?? ""}`;
  const details =
    call?.args !== undefined ||
    result?.model_content ||
    result?.output_id ||
    diff ||
    exitCode !== undefined ||
    !!preview?.lines.length;
  return (
    <ToolCard
      name={name}
      summary={result?.summary || preview?.summary || ""}
      status={
        result ? (result.status === "ok" ? "completed" : "error") : active ? "running" : "unknown"
      }
    >
      {details ? (
        <div className="space-y-2">
          {call?.args !== undefined && (
            <pre className="max-h-36 overflow-auto text-xs whitespace-pre-wrap">
              {visibleOutput(JSON.stringify(call.args, null, 2))}
            </pre>
          )}
          {exitCode !== undefined && <p className="text-xs">Exit code {exitCode}</p>}
          {diff && (
            <pre className="max-h-64 overflow-auto text-xs whitespace-pre-wrap">
              {visibleOutput(diff)}
            </pre>
          )}
          {result?.model_content && (
            <pre className="max-h-64 overflow-auto text-xs whitespace-pre-wrap">
              {visibleOutput(result.model_content)}
            </pre>
          )}
          {result?.output_id && (
            <RetainedOutput key={result.output_id} target={target} id={result.output_id} />
          )}
          {!!preview?.lines.length && (
            <pre className="max-h-48 overflow-auto text-xs whitespace-pre-wrap">
              {preview.lines.map(({ stream, text }, index) => (
                <span key={index} className={stream === "stderr" ? "text-red-400" : undefined}>
                  {visibleOutput(text)}
                </span>
              ))}
            </pre>
          )}
        </div>
      ) : null}
    </ToolCard>
  );
}

function transcriptMessage(entry: Entry) {
  switch (entry.type) {
    case "user":
      return (
        <TurnCard role="user" className="whitespace-pre-wrap break-words">
          {entry.text}
        </TurnCard>
      );
    case "assistant":
      return (
        <TurnCard role="agent">
          {entry.thinking && <ThinkingBlock>{entry.thinking}</ThinkingBlock>}
          {entry.text && <TextContent text={entry.text} />}
        </TurnCard>
      );
    case "question":
    case "permission":
      return (
        <TurnCard role="agent" className="whitespace-pre-wrap">
          {entry.text}
        </TurnCard>
      );
    case "question_reply":
      return (
        <TurnCard role="user" className="whitespace-pre-wrap">
          {entry.text}
          <p className="text-xs text-[var(--color-text-muted)]">Answer · {entry.request_id}</p>
        </TurnCard>
      );
    case "permission_reply":
      return (
        <p className="text-xs text-[var(--color-text-muted)]">
          {entry.allow ? "Allowed" : "Denied"} · {entry.request_id}
        </p>
      );
    case "status":
      return entry.status === "cancelled" && entry.turn !== undefined && entry.turn !== null ? (
        <CancelledNotice turnNumber={entry.turn} />
      ) : (
        <p className="text-xs text-[var(--color-text-muted)]">
          Interrupted{entry.text && ` · ${entry.text}`}
        </p>
      );
    case "error":
      return (
        <ErrorNotice type="server" message={entry.text ?? entry.message}>
          {entry.code}
        </ErrorNotice>
      );
    default:
      return null;
  }
}

export function ConversationTimeline({ target, view }: { target: Target; view: ConversationView }) {
  const calls = new Map<string, ToolCall>();
  const results = new Set<string>();
  for (const entry of view.entries) {
    if (entry.type === "tool_call") calls.set(callKey(entry.turn, entry.tool_call_id), entry);
    if (entry.type === "tool_result") results.add(callKey(entry.turn, entry.tool_call_id));
  }
  const preview = view.preview;
  const active = view.snapshot?.agent;
  const activeTurn =
    active?.status === "running" || active?.status === "waiting" ? active.turn : undefined;
  return (
    <>
      {view.entries.map((entry) => {
        if (entry.type === "tool_call" || entry.type === "tool_result") {
          const key = callKey(entry.turn, entry.tool_call_id);
          if (entry.type === "tool_call" && results.has(key)) return null;
          const progress =
            entry.turn === preview?.turn
              ? preview?.tools.find(({ id }) => id === entry.tool_call_id)
              : undefined;
          return (
            <div key={`tool-${key}`}>
              <Tool
                target={target}
                call={calls.get(key)}
                result={entry.type === "tool_result" ? entry : undefined}
                preview={progress}
                active={entry.turn === activeTurn || progress !== undefined}
              />
            </div>
          );
        }
        return <div key={entry.id}>{transcriptMessage(entry)}</div>;
      })}
      {preview?.tools
        .filter(
          ({ id }) =>
            !calls.has(callKey(preview.turn, id)) && !results.has(callKey(preview.turn, id)),
        )
        .map((tool) => (
          <Tool key={tool.id} target={target} preview={tool} />
        ))}
      {(preview?.text || preview?.thinking) && (
        <TurnCard role="agent">
          {preview.thinking && <ThinkingBlock>{preview.thinking}</ThinkingBlock>}
          {preview.text && <TextContent text={preview.text} />}
        </TurnCard>
      )}
    </>
  );
}
