import { config } from "../config";
import { backoffMs, shouldRetry } from "../retry";
import { sign } from "./signature";

export type Delivery = { merchantId: string; url: string; secret: string; event: unknown };

/** 이벤트 하나를 가맹점에 보내요. 실패하면 backoffMs만큼 기다렸다가 config.retry.max번까지 다시 보내요. */
export async function deliver(d: Delivery): Promise<"delivered" | "dead_letter"> {
  const body = JSON.stringify(d.event);
  for (let attempt = 0; ; attempt++) {
    if (attempt > 0) await sleep(backoffMs(attempt));
    const status = await post(d.url, body, sign(body, d.secret));
    if (typeof status === "number" && status >= 200 && status < 300) return "delivered";
    if (!shouldRetry(attempt + 1, status)) return "dead_letter";
  }
}

async function post(url: string, body: string, signature: string): Promise<number | "timeout" | "network"> {
  try {
    const res = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", "x-paymint-signature": signature },
      body,
      signal: AbortSignal.timeout(config.webhook.timeoutMs),
    });
    return res.status;
  } catch (e) {
    return (e as Error).name === "TimeoutError" ? "timeout" : "network";
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
