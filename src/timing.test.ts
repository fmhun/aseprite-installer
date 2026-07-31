import { afterEach, describe, expect, it, vi } from "vitest";
import { withMinimumDuration } from "./timing";

describe("withMinimumDuration", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("keeps a fast successful operation pending for the requested duration", async () => {
    vi.useFakeTimers();
    const result = withMinimumDuration(Promise.resolve("done"), 2_000);
    let settled = false;
    void result.then(() => {
      settled = true;
    });

    await vi.advanceTimersByTimeAsync(1_999);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);

    await expect(result).resolves.toBe("done");
    expect(settled).toBe(true);
  });

  it("also preserves the loading duration when the operation fails", async () => {
    vi.useFakeTimers();
    const result = withMinimumDuration(
      Promise.reject(new Error("network failed")),
      2_000,
    );
    const assertion = expect(result).rejects.toThrow("network failed");

    await vi.advanceTimersByTimeAsync(2_000);
    await assertion;
  });
});
