import { config } from "./config";

/** n번째 재시도(1부터) 전에 기다릴 시간. 두 배씩 늘리고 ±20% 지터를 섞어요. */
export function backoffMs(attempt: number, base = config.retry.baseMs, jitter = config.retry.jitter): number {
  const plain = base * 2 ** (attempt - 1);
  const spread = plain * jitter;
  return Math.round(plain - spread + Math.random() * spread * 2);
}

/** 실패한 전송을 다시 보낼지. 4xx 중 408·429만 다시 보내고, 나머지 4xx는 가맹점 설정 문제라 바로 포기해요. */
export function shouldRetry(attempt: number, status: number | "timeout" | "network"): boolean {
  if (attempt > config.retry.max) return false;
  if (status === "timeout" || status === "network") return true;
  if (status === 408 || status === 429) return true;
  return status >= 500;
}
