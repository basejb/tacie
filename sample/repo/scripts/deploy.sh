#!/usr/bin/env bash
# 사용법: ./scripts/deploy.sh <staging|production> [--rollback]
# 서버를 한 대씩 새 이미지로 바꾸고, /healthz가 200이면 다음 서버로 넘어가요.
# --rollback은 직전에 성공한 이미지 태그로 다시 배포해요. 마이그레이션은 되돌리지 않아요.
set -euo pipefail
env="$1"; shift
tag=$(git rev-parse --short HEAD)
if [[ "${1:-}" == "--rollback" ]]; then tag=$(cat ".last-good-$env"); fi
for host in $(cat "deploy/$env.hosts"); do
  ssh "$host" "docker pull paymint:$tag && docker compose up -d --no-deps api"
  until curl -fs "http://$host:8080/healthz" >/dev/null; do sleep 2; done
done
echo "$tag" > ".last-good-$env"
