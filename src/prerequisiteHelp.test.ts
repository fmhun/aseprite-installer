import { describe, expect, it } from "vitest";
import { getPrerequisiteGuide } from "./prerequisiteHelp";

describe("prerequisite help guides", () => {
  it.each([
    "nonElevated",
    "macos",
    "architecture",
    "translation",
    "xcode",
    "sdk",
    "baseline",
    "cmake",
    "ninja",
    "curl",
    "unzip",
    "skiaProxy",
    "buildPath",
    "caseSensitiveBuild",
    "workspace",
    "destination",
    "targetState",
    "asepriteClosed",
    "disk",
    "toolchain",
  ])(
    "provides actionable, sourced guidance for %s",
    (id) => {
      const guide = getPrerequisiteGuide(id);

      expect(guide.title).not.toBe("");
      expect(guide.summary).not.toBe("");
      expect(guide.steps.length).toBeGreaterThan(0);
      expect(guide.links.length).toBeGreaterThan(0);
      expect(guide.links.every((link) => link.url.startsWith("https://"))).toBe(true);
    },
  );

  it("keeps upstream path-shape failures separate from permission failures", () => {
    const buildPath = getPrerequisiteGuide("buildPath");
    const workspace = getPrerequisiteGuide("workspace");

    expect(buildPath.summary).toContain("space, tab, or line break");
    expect(buildPath.summary).toContain("build.sh");
    expect(workspace.summary).toContain("fsync");
    expect(workspace.summary).toContain("extended attributes");
    expect(workspace.summary).not.toContain("whitespace");
  });

  it("explains why a case-sensitive build volume is blocked", () => {
    const guide = getPrerequisiteGuide("caseSensitiveBuild");

    expect(guide.summary).toContain("Aseprite.app");
    expect(guide.summary).toContain("aseprite.app");
    expect(guide.links.some((link) => link.url.includes("disk-utility"))).toBe(true);
  });

  it("falls back to the official Aseprite build guide", () => {
    const guide = getPrerequisiteGuide("future-platform-tool");

    expect(guide.links[0]?.url).toBe(
      "https://github.com/aseprite/aseprite/blob/main/INSTALL.md",
    );
  });
});
