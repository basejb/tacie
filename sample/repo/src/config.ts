// 설정 기본값. 환경 변수가 있으면 그 값을 써요 (docs/설정.md).

const num = (name: string, fallback: number): number => {
  const raw = process.env[name];
  if (raw === undefined || raw === "") return fallback;
  const n = Number(raw);
  if (!Number.isFinite(n) || n < 0) throw new Error(`${name} must be a non-negative number, got "${raw}"`);
  return n;
};

export const config = {
  port: num("PORT", 8080),
  databaseUrl: process.env.DATABASE_URL ?? "",
  redisUrl: process.env.REDIS_URL ?? "redis://localhost:6379",
  retry: {
    // 첫 전송 뒤 다시 보내는 최대 횟수 (결정 0001)
    max: num("PAYMINT_RETRY_MAX", 3),
    // 0.5초에서 시작해 두 배씩: 0.5, 1, 2초
    baseMs: num("PAYMINT_RETRY_BASE_MS", 500),
    jitter: 0.2,
  },
  webhook: {
    // 가맹점 서버 응답을 기다리는 시간. 넘기면 실패로 보고 재시도해요.
    timeoutMs: num("PAYMINT_WEBHOOK_TIMEOUT_MS", 3000),
  },
  signingSecret: process.env.PAYMINT_SIGNING_SECRET ?? "dev-secret-change-me",
};

if (!config.databaseUrl) throw new Error("DATABASE_URL is required");
