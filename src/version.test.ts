import { describe, expect, it } from "vitest";
import { compareVersions, normalizeVersion } from "./version";

describe("version helpers", () => {
  it("normalizes Aseprite tags", () => {
    expect(normalizeVersion("v1.3.18.1")).toEqual([1, 3, 18, 1]);
  });

  it("orders update, downgrade and reinstall choices", () => {
    expect(compareVersions("v1.3.18.1", "v1.3.17.2")).toBe(1);
    expect(compareVersions("v1.3.17.2", "v1.3.18.1")).toBe(-1);
    expect(compareVersions("v1.3.18.1", "v1.3.18.1")).toBe(0);
  });
});
