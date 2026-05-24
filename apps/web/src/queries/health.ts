import { useQuery } from "@tanstack/react-query";

export interface HealthResponse {
  ok: boolean;
  version: string;
}

export function useHealth() {
  return useQuery<HealthResponse>({
    queryKey: ["health"],
    queryFn: () => fetch("/health").then((r) => r.json() as Promise<HealthResponse>),
    retry: false,
    staleTime: 30_000,
  });
}
