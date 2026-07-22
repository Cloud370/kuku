import { WebApiError } from '../../api/client';

const serverBusyRetryDelays = [50, 100, 200] as const;

export async function retryReviewRead<T>(operation: () => Promise<T>): Promise<T> {
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await operation();
    } catch (error) {
      const delay = serverBusyRetryDelays[attempt];
      if (!(error instanceof WebApiError) || error.code !== 'server_busy' || delay === undefined) {
        throw error;
      }
      await new Promise<void>((resolve) => {
        window.setTimeout(resolve, delay);
      });
    }
  }
}
