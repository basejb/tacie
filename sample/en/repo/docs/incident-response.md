# Incident response

## Webhooks keep failing

1. Open "Failure rate by merchant" on the dashboard and check whether it's just one merchant.
2. If only one merchant is failing, it's usually their server. Events that fail all 3 retries go to the `dead_letters` table. Once the merchant is back up, replay them with `POST /admin/dead-letters/replay?merchant=<id>`.
3. If every merchant is failing, the problem is on our side. Check the Redis connection and the latest deploy first. If the latest deploy looks suspicious, [roll back](deploy.md#rolling-back).

## Alerts

When the failure rate stays above 5% for 5 minutes, an alert goes to #paymint-alerts and the on-call engineer gets paged. The on-call rotation is on the team calendar.
