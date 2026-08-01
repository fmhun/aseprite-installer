export const DEMO_DURATION = 16_000;

export type DemoPhase =
  | "status"
  | "release"
  | "preflight"
  | "build"
  | "complete";

export type BuildStage = "download" | "verify" | "compile" | "sign" | "install";

export type DemoFrame = {
  phase: DemoPhase;
  progress: number;
  buildStage: BuildStage;
  clicking: boolean;
};

const clickWindows = [
  [1_720, 2_060],
  [4_620, 4_940],
  [7_520, 7_820],
] as const;

function normalizeElapsed(elapsed: number): number {
  if (!Number.isFinite(elapsed) || elapsed < 0) return 0;
  return elapsed % DEMO_DURATION;
}

export function getDemoFrame(elapsed: number, reducedMotion = false): DemoFrame {
  const time = reducedMotion ? DEMO_DURATION - 1 : normalizeElapsed(elapsed);
  const phase: DemoPhase =
    time < 2_200
      ? "status"
      : time < 5_200
        ? "release"
        : time < 8_000
          ? "preflight"
          : time < 14_300
            ? "build"
            : "complete";

  const progress =
    phase === "complete"
      ? 100
      : phase === "build"
        ? Math.min(99, Math.round(((time - 8_000) / 6_300) * 100))
        : 0;

  const buildStage: BuildStage =
    progress < 18
      ? "download"
      : progress < 31
        ? "verify"
        : progress < 76
          ? "compile"
          : progress < 89
            ? "sign"
            : "install";

  return {
    phase,
    progress,
    buildStage,
    clicking: !reducedMotion && clickWindows.some(([start, end]) => time >= start && time <= end),
  };
}
