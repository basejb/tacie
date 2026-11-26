# Changelog

## 1.4.0 (2025-03-14)
- Retries cut from 5 to 3, with the delay starting at 0.5 s and doubling each time ([ADR 0001](docs/decisions/0001-retry-policy.md)).
- Events that fail all 3 retries now land in `dead_letters` and can be replayed through the admin API.

## 1.3.0 (2025-01-22)
- Moved the event queue from Postgres polling to Redis Streams ([ADR 0002](docs/decisions/0002-queue-choice.md)).
