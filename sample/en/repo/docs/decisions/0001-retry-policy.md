# 0001. Retry policy: at most 3 retries, doubling from 0.5 s

- Status: accepted (2025-03-12)
- Decided by: Payments Platform team

## Context

Merchant servers often go down for a moment and come right back. Before we had retries, merchants who missed a payment-complete webhook were contacting us about it several times a week.

## Decision

- On failure, retry at most 3 times (`PAYMINT_RETRY_MAX=3`).
- The delay starts at 0.5 s and doubles (0.5 s, 1 s, 2 s), with ±20% jitter so lots of events don't retry at the same instant.
- After 3 failed retries the event goes to `dead_letters`, and a person replays it.

## Alternatives considered

- 5 retries: a single failing merchant backed up the whole queue (the February 2025 incident). That's why we cut it down.
- A fixed 1 s delay: requests piled onto the merchant the moment it came back.
