import { postStream } from "./client";

export async function createRun(
  prompt: string,
  workspace: string,
  sessionId: string | undefined,
  onLine: (line: string) => void,
  onDone: (sessionId: string) => void,
  onError: (err: Error) => void,
): Promise<void> {
  const body: Record<string, unknown> = { prompt, workspace };
  if (sessionId) body.session_id = sessionId;

  try {
    const stream = await postStream("/runs", body);
    const reader = stream.getReader();
    const decoder = new TextDecoder();
    let buffer = "";

    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split("\n");
        buffer = lines.pop() ?? "";
        for (const line of lines) {
          if (!line.trim()) continue;
          try {
            const parsed = JSON.parse(line) as Record<string, unknown>;
            if (parsed.type === "run_start") {
              onDone(parsed.run_id as string);
            }
            onLine(line);
          } catch {
            // skip malformed lines
          }
        }
      }
    } finally {
      reader.releaseLock();
    }
  } catch (e) {
    onError(e instanceof Error ? e : new Error(String(e)));
  }
}
