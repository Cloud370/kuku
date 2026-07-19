export function sendResponse(
  _runId: string,
  _interactionId: string,
  _choice: string,
): Promise<{ ok: boolean }> {
  return Promise.reject(new Error("legacy Run response API removed"));
}
