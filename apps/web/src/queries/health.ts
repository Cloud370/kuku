import { useQuery } from "@tanstack/react-query";
import { ApiError, errorMessage, exchangeBrowserTicket, get } from "@/api/client";

export interface HealthResponse {
  ok: true;
  version: string;
  protocol_version?: number;
  management_version: number;
}

interface ServiceStatus {
  version: string;
  protocol_version: number;
  management_version: number;
  config_path: string | null;
  instance_id: string | null;
  busy: string[];
  draining: boolean;
}

const supportedProtocol = 3;

export function useHealth() {
  return useQuery({
    queryKey: ["health"],
    queryFn: async ({ signal }) => {
      const ticketError = await exchangeBrowserTicket().then(() => undefined, errorMessage);
      const health = await get<HealthResponse>("/health", signal, false);
      const mismatch =
        health.protocol_version !== supportedProtocol
          ? `kuku ${health.version} uses protocol ${String(health.protocol_version ?? "unknown")}. This browser interface uses protocol ${String(supportedProtocol)}.`
          : undefined;
      try {
        const status = await get<ServiceStatus>("/service/status", signal);
        return { ...status, mismatch };
      } catch (error) {
        if (mismatch) throw new ApiError(409, "protocol_mismatch", mismatch);
        if (error instanceof ApiError && error.status === 401 && ticketError) {
          throw new ApiError(401, "auth_required", ticketError);
        }
        throw error;
      }
    },
    retry: false,
    staleTime: 30_000,
  });
}
