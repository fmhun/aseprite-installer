import { describe, expect, it } from "vitest";
import { DEMO_DURATION, getDemoFrame } from "./demo";

describe("getDemoFrame", () => {
  it("moves through the complete installation story", () => {
    expect(getDemoFrame(0).phase).toBe("status");
    expect(getDemoFrame(2_200).phase).toBe("release");
    expect(getDemoFrame(5_000).phase).toBe("preflight");
    expect(getDemoFrame(7_400).phase).toBe("eula");
    expect(getDemoFrame(9_300).phase).toBe("build");
    expect(getDemoFrame(14_300).phase).toBe("complete");
  });

  it("reports useful build stages and reaches completion", () => {
    expect(getDemoFrame(9_500)).toMatchObject({ buildStage: "download" });
    expect(getDemoFrame(10_700)).toMatchObject({ buildStage: "verify" });
    expect(getDemoFrame(12_000)).toMatchObject({ buildStage: "compile" });
    expect(getDemoFrame(13_600)).toMatchObject({ buildStage: "sign" });
    expect(getDemoFrame(14_200)).toMatchObject({ buildStage: "install" });
    expect(getDemoFrame(15_000)).toMatchObject({ phase: "complete", progress: 100 });
  });

  it("loops after sixteen seconds", () => {
    expect(getDemoFrame(DEMO_DURATION)).toEqual(getDemoFrame(0));
    expect(getDemoFrame(DEMO_DURATION + 5_000)).toEqual(getDemoFrame(5_000));
  });

  it("uses the final static frame when reduced motion is requested", () => {
    expect(getDemoFrame(0, true)).toMatchObject({
      phase: "complete",
      progress: 100,
      clicking: false,
    });
  });
});
