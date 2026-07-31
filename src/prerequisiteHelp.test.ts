import { describe, expect, it } from "vitest";
import { getPrerequisiteGuide } from "./prerequisiteHelp";

describe("prerequisite help guides", () => {
  it.each(["macos", "xcode", "sdk", "cmake", "ninja", "disk"])(
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

  it("falls back to the official Aseprite build guide", () => {
    const guide = getPrerequisiteGuide("future-platform-tool");

    expect(guide.links[0]?.url).toBe(
      "https://github.com/aseprite/aseprite/blob/main/INSTALL.md",
    );
  });
});
