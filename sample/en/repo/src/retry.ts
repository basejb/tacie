import { config } from "./config";

/** Delay before the nth retry (1-based). Doubles each time, with ±20% jitter. */
export function backoffMs(attempt: number, base = config.retry.baseMs, jitter = config.retry.jitter): number {
  const plain = base * 2 ** (attempt - 1);
  const spread = plain * jitter;
  return Math.round(plain - spread + Math.random() * spread * 2);
}

/** Whether to retry a failed delivery. Of the 4xx codes only 408 and 429 are retried; any other 4xx means the merchant is misconfigured, so we give up right away. */
export function shouldRetry(attempt: number, status: number | "timeout" | "network"): boolean {
  if (attempt > config.retry.max) return false;
  if (status === "timeout" || status === "network") return true;
  if (status === 408 || status === 429) return true;
  return status >= 500;
}
