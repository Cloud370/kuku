import { post } from "./client";

export function sendResponse(
  runId: string,
  interactionId: string,
  choice: string,
): Promise<{ ok: boolean }> {
  return post(`/runs/${runId}/responses`, { interaction_id: interactionId, choice });
}
