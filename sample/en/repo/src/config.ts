// Default settings. An environment variable, when set, wins (docs/configuration.md).

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
    // Max retries after the first attempt (ADR 0001)
    max: num("PAYMINT_RETRY_MAX", 3),
    // Starts at 0.5 s and doubles: 0.5, 1, 2 s
    baseMs: num("PAYMINT_RETRY_BASE_MS", 500),
    jitter: 0.2,
  },
  webhook: {
    // How long to wait for the merchant to respond. Past that, it counts as a failure and is retried.
    timeoutMs: num("PAYMINT_WEBHOOK_TIMEOUT_MS", 3000),
  },
  signingSecret: process.env.PAYMINT_SIGNING_SECRET ?? "dev-secret-change-me",
};

if (!config.databaseUrl) throw new Error("DATABASE_URL is required");
