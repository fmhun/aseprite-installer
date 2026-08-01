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

  it("orders stable, release candidate, and beta builds", () => {
    expect(compareVersions("v1.3.16", "v1.3.16-rc2")).toBe(1);
    expect(compareVersions("v1.3.16-rc2", "v1.3.16-beta99")).toBe(1);
    expect(compareVersions("v1.3.16-beta3", "v1.3.16-beta2")).toBe(1);
    expect(compareVersions("v1.3.16-rc1", "v1.3.16-rc2")).toBe(-1);
    expect(compareVersions("v1.3.16-beta3", "v1.3.16-beta3")).toBe(0);
  });

  it("orders the numeric release before prerelease precedence", () => {
    expect(compareVersions("v1.3.17-beta1", "v1.3.16")).toBe(1);
    expect(compareVersions("v1.3.16-rc9", "v1.3.16.1-beta1")).toBe(-1);
  });
});
