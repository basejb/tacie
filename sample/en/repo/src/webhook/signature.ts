import { createHmac, timingSafeEqual } from "node:crypto";

/**
 * Signs a webhook body. Merchants receive it in the `X-Paymint-Signature` header.
 * Format: `t=<unix seconds>,v1=<hex>`. The signed payload is `${t}.${body}`, using HMAC-SHA256.
 * Merchants are told to reject a t older than 5 minutes (replay protection).
 */
export function sign(body: string, secret: string, now = Math.floor(Date.now() / 1000)): string {
  const mac = createHmac("sha256", secret).update(`${now}.${body}`).digest("hex");
  return `t=${now},v1=${mac}`;
}

export function verify(header: string, body: string, secret: string, now = Math.floor(Date.now() / 1000)): boolean {
  const parts = Object.fromEntries(header.split(",").map((p) => p.split("=") as [string, string]));
  const t = Number(parts.t);
  if (!Number.isFinite(t) || Math.abs(now - t) > 300) return false;
  const expected = Buffer.from(sign(body, secret, t).split("v1=")[1], "hex");
  const got = Buffer.from(parts.v1 ?? "", "hex");
  return got.length === expected.length && timingSafeEqual(got, expected);
}
