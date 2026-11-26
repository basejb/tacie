# Deploying and rolling back

## Deploying

1. Merging to `main` makes CI build an image and deploy it to staging automatically.
2. Check that `npm run test:e2e -- --env staging` passes against staging.
3. Production deploys are done by a person: `npm run deploy:prod`. The deploy script (`scripts/deploy.sh`) swaps in the new version one server at a time and moves on once that server's `/healthz` returns 200.
4. Whoever deployed posts the version and a short summary of changes in #paymint-deploy.

No production deploys after 4 pm on Fridays.

## Rolling back

If something goes wrong, go straight back to the previous version:

```bash
./scripts/deploy.sh production --rollback
```

`--rollback` redeploys the last image tag that deployed successfully. That alone isn't enough for a deploy that included a DB migration: run the migration's `down` first. PRs with a migration get `[migration]` in the title so they're easy to spot.
