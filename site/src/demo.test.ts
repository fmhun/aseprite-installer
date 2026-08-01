import { describe, expect, it } from "vitest";
import { DEMO_DURATION, getDemoFrame } from "./demo";

describe("getDemoFrame", () => {
  it("moves through the complete installation story", () => {
    expect(getDemoFrame(0).phase).toBe("status");
    expect(getDemoFrame(2_200).phase).toBe("release");
    expect(getDemoFrame(5_200).phase).toBe("preflight");
    expect(getDemoFrame(8_000).phase).toBe("build");
    expect(getDemoFrame(14_300).phase).toBe("complete");
  });

  it("reports useful build stages and reaches completion", () => {
    expect(getDemoFrame(8_200)).toMatchObject({ buildStage: "download" });
    expect(getDemoFrame(9_300)).toMatchObject({ buildStage: "verify" });
    expect(getDemoFrame(10_500)).toMatchObject({ buildStage: "compile" });
    expect(getDemoFrame(13_200)).toMatchObject({ buildStage: "validate" });
    expect(getDemoFrame(14_000)).toMatchObject({ buildStage: "install" });
    expect(getDemoFrame(15_000)).toMatchObject({ phase: "complete", progress: 100 });
  });

  it("loops after sixteen seconds", () => {
    expect(getDemoFrame(DEMO_DURATION)).toEqual(getDemoFrame(0));
    expect(getDemoFrame(DEMO_DURATION + 5_200)).toEqual(getDemoFrame(5_200));
  });

  it("uses the final static frame when reduced motion is requested", () => {
    expect(getDemoFrame(0, true)).toMatchObject({
      phase: "complete",
      progress: 100,
      clicking: false,
    });
  });
});
