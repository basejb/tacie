# 0002. Use Redis Streams for the event queue

- Status: accepted (2025-01-20)

## Decision

The event queue is Redis Streams. A consumer group splits the work across servers, and messages that weren't processed are picked up again via `XPENDING`.

## Why not SQS

- We handle around 100k events a day, which one Redis instance covers easily.
- We already run Redis for sessions, so there's nothing new to operate.
- Keeping events in order per merchant was simpler.

Revisit this if we go past 1M events a day or need multiple regions.
