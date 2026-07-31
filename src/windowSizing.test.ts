import { describe, expect, it } from "vitest";
import {
  DEFAULT_WINDOW_HEIGHT,
  calculateWindowHeight,
} from "./windowSizing";

describe("calculateWindowHeight", () => {
  it("uses the same fixed height for every application view", () => {
    expect(calculateWindowHeight(DEFAULT_WINDOW_HEIGHT, 900)).toBe(680);
  });

  it("keeps compact screens at the minimum height", () => {
    expect(calculateWindowHeight(280, 900)).toBe(420);
  });

  it("fits the content when it is between the limits", () => {
    expect(calculateWindowHeight(562.2, 900)).toBe(563);
  });

  it("keeps the window inside the monitor work area", () => {
    expect(calculateWindowHeight(1_200, 800)).toBe(776);
  });
});
