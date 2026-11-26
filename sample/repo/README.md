# 페이민트

결제 대행사에서 받은 결제 이벤트를 가맹점 서버로 웹훅으로 전달하는 중계 서비스예요. 이벤트를 큐(Redis Streams)에 넣고, 가맹점 서버가 응답하지 않으면 정해진 횟수만큼 다시 보내요.

## 로컬에서 실행하기

Node.js 20과 Docker가 필요해요.

```bash
cp .env.example .env
docker compose up -d redis postgres
npm install
npm run dev
```

서버는 `http://localhost:8080`에서 떠요. 상태 확인은 `GET /healthz`예요.

## 테스트

```bash
npm test            # 단위 테스트 (vitest)
npm run test:e2e    # 가짜 가맹점 서버를 띄우고 웹훅 전체 흐름을 확인해요
```

PR을 올리면 CI가 두 가지를 모두 돌려요. e2e는 Redis가 필요해서 `docker compose up -d redis`를 먼저 해야 해요.

## 더 보기

- 설정 값과 환경 변수: [docs/설정.md](docs/설정.md)
- 배포와 롤백: [docs/배포.md](docs/배포.md)
- 장애가 났을 때: [docs/장애_대응.md](docs/장애_대응.md)
- 왜 이렇게 정했는지: [docs/결정](docs/결정)
