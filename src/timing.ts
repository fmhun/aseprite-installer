export const MINIMUM_LOADING_DURATION_MS = 2_000;

function delay(durationMs: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, durationMs));
}

export async function withMinimumDuration<T>(
  operation: Promise<T>,
  durationMs = MINIMUM_LOADING_DURATION_MS,
): Promise<T> {
  const [result] = await Promise.allSettled([
    operation,
    delay(Math.max(0, durationMs)),
  ]);

  if (result.status === "rejected") throw result.reason;
  return result.value;
}
