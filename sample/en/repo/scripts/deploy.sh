#!/usr/bin/env bash
# Usage: ./scripts/deploy.sh <staging|production> [--rollback]
# Swaps servers to the new image one at a time; moves to the next once /healthz returns 200.
# --rollback redeploys the last image tag that succeeded. It does not undo migrations.
set -euo pipefail
env="$1"; shift
tag=$(git rev-parse --short HEAD)
if [[ "${1:-}" == "--rollback" ]]; then tag=$(cat ".last-good-$env"); fi
for host in $(cat "deploy/$env.hosts"); do
  ssh "$host" "docker pull paymint:$tag && docker compose up -d --no-deps api"
  until curl -fs "http://$host:8080/healthz" >/dev/null; do sleep 2; done
done
echo "$tag" > ".last-good-$env"
