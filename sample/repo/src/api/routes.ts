import type { FastifyInstance } from "fastify";
import { replayDeadLetters, updateMerchant } from "../db";

export async function routes(app: FastifyInstance) {
  app.get("/healthz", async () => ({ ok: true }));

  // 관리자 API. 사내망에서만 열려 있고, 헤더 X-Admin-Token이 필요해요.
  app.patch("/admin/merchants/:id", async (req) => updateMerchant((req.params as { id: string }).id, req.body));
  app.post("/admin/dead-letters/replay", async (req) => {
    const merchant = (req.query as { merchant?: string }).merchant;
    return { replayed: await replayDeadLetters(merchant) };
  });
}
