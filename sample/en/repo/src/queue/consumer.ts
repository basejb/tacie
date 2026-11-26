import Redis from "ioredis";
import { config } from "../config";
import { deliver } from "../webhook/sender";
import { saveDeadLetter, merchantOf } from "../db";

const STREAM = "paymint:events";
const GROUP = "senders";

/** Reads events through a Redis Streams consumer group (ADR 0002). Each server uses its own consumer name. */
export async function run(consumer: string) {
  const redis = new Redis(config.redisUrl);
  await redis.xgroup("CREATE", STREAM, GROUP, "$", "MKSTREAM").catch(() => {});
  for (;;) {
    const res = (await redis.xreadgroup("GROUP", GROUP, consumer, "COUNT", 10, "BLOCK", 5000, "STREAMS", STREAM, ">")) as
      | [string, [string, string[]][]][]
      | null;
    for (const [, messages] of res ?? []) {
      for (const [id, fields] of messages) {
        const event = JSON.parse(fields[1]);
        const merchant = await merchantOf(event.merchantId);
        const result = await deliver({ merchantId: merchant.id, url: merchant.webhookUrl, secret: merchant.secret, event });
        if (result === "dead_letter") await saveDeadLetter(merchant.id, event);
        await redis.xack(STREAM, GROUP, id);
      }
    }
  }
}
