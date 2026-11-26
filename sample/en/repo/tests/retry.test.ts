import { describe, expect, it } from "vitest";
import { backoffMs, shouldRetry } from "../src/retry";

describe("retry", () => {
  it("doubles from the base with ±20% jitter", () => {
    for (const [attempt, plain] of [[1, 500], [2, 1000], [3, 2000]]) {
      const ms = backoffMs(attempt, 500, 0.2);
      expect(ms).toBeGreaterThanOrEqual(plain * 0.8);
      expect(ms).toBeLessThanOrEqual(plain * 1.2);
    }
  });

  it("gives up after the third retry", () => {
    expect(shouldRetry(3, 503)).toBe(true);
    expect(shouldRetry(4, 503)).toBe(false);
  });

  it("does not retry a merchant's 4xx except 408 and 429", () => {
    expect(shouldRetry(1, 404)).toBe(false);
    expect(shouldRetry(1, 429)).toBe(true);
  });
});
