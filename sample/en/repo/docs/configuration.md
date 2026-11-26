# Configuration

Every setting can be overridden with an environment variable. Anything you don't set falls back to the default in `src/config.ts`.

## Server and storage

| Variable | Default | What it does |
| --- | --- | --- |
| `PORT` | 8080 | API server port |
| `DATABASE_URL` | (none, required) | Postgres for merchant records and delivery history |
| `REDIS_URL` | `redis://localhost:6379` | Event queue (Redis Streams) |

## Retries and timeouts

| Variable | Default | What it does |
| --- | --- | --- |
| `PAYMINT_RETRY_MAX` | 3 | Max number of retries when a merchant's server fails. The first attempt doesn't count |
| `PAYMINT_RETRY_BASE_MS` | 500 | Starting retry delay. Doubles each time: 0.5 s, 1 s, 2 s |
| `PAYMINT_WEBHOOK_TIMEOUT_MS` | 3000 | How long to wait for the merchant's server to respond (3 s). Past that, it counts as a failure and is retried |

Why the retry count is 3: see [ADR 0001](decisions/0001-retry-policy.md).

## Signing and per-merchant settings

| Variable | Default | What it does |
| --- | --- | --- |
| `PAYMINT_SIGNING_SECRET` | (dev only) | Fallback signing secret. In production each merchant's secret comes from the DB |

Each merchant's webhook URL and signing secret live in the `merchants` table. Change them with the admin API: `PATCH /admin/merchants/:id`.
