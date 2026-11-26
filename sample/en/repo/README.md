# PayMint

PayMint relays payment events from our payment gateway to merchant servers as webhooks. Every event goes onto a queue (Redis Streams), and if a merchant's server doesn't answer, PayMint retries a fixed number of times.

## Running locally

You'll need Node.js 20 and Docker.

```bash
cp .env.example .env
docker compose up -d redis postgres
npm install
npm run dev
```

The server listens on `http://localhost:8080`. Health check: `GET /healthz`.

## Tests

```bash
npm test            # unit tests (vitest)
npm run test:e2e    # spins up a fake merchant server and runs the whole webhook flow
```

CI runs both on every PR. The e2e suite needs Redis, so run `docker compose up -d redis` first.

## Docs

- Settings and environment variables: [docs/configuration.md](docs/configuration.md)
- Deploying and rolling back: [docs/deploy.md](docs/deploy.md)
- When things break: [docs/incident-response.md](docs/incident-response.md)
- Why we built it this way: [docs/decisions](docs/decisions)
