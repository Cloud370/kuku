export function createRun(
  _prompt: string,
  _workspace: string,
  _sessionId: string | undefined,
  _onLine: (line: string) => void,
  _onDone: (sessionId: string) => void,
  _onError: (err: Error) => void,
): Promise<void> {
  return Promise.reject(new Error("legacy Run API removed"));
}
