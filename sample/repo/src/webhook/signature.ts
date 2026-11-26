import { createHmac, timingSafeEqual } from "node:crypto";

/**
 * 웹훅 본문에 서명을 붙여요. 가맹점은 `X-Paymint-Signature` 헤더로 받아요.
 * 형식: `t=<유닉스 초>,v1=<hex>`. 서명 대상은 `${t}.${body}`이고 HMAC-SHA256이에요.
 * 5분보다 오래된 t는 가맹점이 거절하게 안내해요(재전송 공격 방지).
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
