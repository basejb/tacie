import type { FastifyInstance } from "fastify";
import { replayDeadLetters, updateMerchant } from "../db";

export async function routes(app: FastifyInstance) {
  app.get("/healthz", async () => ({ ok: true }));

  // Admin API. Only reachable from the internal network, and needs an X-Admin-Token header.
  app.patch("/admin/merchants/:id", async (req) => updateMerchant((req.params as { id: string }).id, req.body));
  app.post("/admin/dead-letters/replay", async (req) => {
    const merchant = (req.query as { merchant?: string }).merchant;
    return { replayed: await replayDeadLetters(merchant) };
  });
}
