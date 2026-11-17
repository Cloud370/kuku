import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { fetchOutput } from "@/api/sessions";
import type { Target } from "@/api/sessions";
import { errorMessage } from "@/api/client";
import { visibleOutput } from "@/lib/output";
import { Button } from "@/components/ui/Button";

export function RetainedOutput({ target, id }: { target: Target; id: string }) {
  const [offsets, setOffsets] = useState<number[]>([]);
  const offset = offsets.at(-1);
  const page = useQuery({
    queryKey: ["output", target.workspace, target.session, id, offset],
    queryFn: ({ signal }) => fetchOutput(target, id, offset ?? 0, signal),
    enabled: offset !== undefined,
    staleTime: 60_000,
    gcTime: 0,
    retry: false,
  });
  const data = page.data;
  const next = data?.has_more ? data.next_offset : undefined;

  return (
    <div className="space-y-1 text-xs">
      {data && (
        <>
          <p className="text-[var(--color-text-muted)]">
            Bytes {data.offset}–{data.next_offset} of {data.total_bytes}
          </p>
          {data.capture_truncated && <p>Output reached the retention limit.</p>}
          {data.cleanup_incomplete && <p>Output capture ended during cleanup.</p>}
          {data.lossy_utf8 && <p>Some bytes could not be displayed as UTF-8.</p>}
          <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words font-mono">
            {data.segments.map((segment) => (
              <span
                key={segment.offset}
                className={segment.stream === "stderr" ? "text-red-400" : undefined}
              >
                {visibleOutput(segment.text)}
              </span>
            ))}
          </pre>
        </>
      )}
      {page.isFetching && <p role="status">Loading output…</p>}
      {page.error && (
        <p role="alert" className="text-red-400">
          {errorMessage(page.error)}
        </p>
      )}
      <div className="flex gap-2">
        {offsets.length > 1 && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setOffsets((items) => items.slice(0, -1));
            }}
          >
            Previous output
          </Button>
        )}
        {offset === undefined && (
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              setOffsets([0]);
            }}
          >
            Load output
          </Button>
        )}
        {next !== undefined && (
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              setOffsets((items) => [...items, next]);
            }}
          >
            Load more output
          </Button>
        )}
        {page.error && (
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              void page.refetch();
            }}
          >
            Retry output
          </Button>
        )}
      </div>
    </div>
  );
}
