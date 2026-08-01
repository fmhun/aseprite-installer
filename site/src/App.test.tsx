import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App, { ProductDemo } from "./App";

const matchMedia = (matches: boolean) =>
  vi.fn().mockReturnValue({
    matches,
    media: "(prefers-reduced-motion: reduce)",
    onchange: null,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  });

describe("landing page", () => {
  beforeEach(() => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      writable: true,
      value: matchMedia(false),
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("presents the product, direct download, and open-source links", () => {
    render(<App />);

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      "Build Asepritefrom source. Locally.",
    );
    const downloads = screen.getAllByRole("link", { name: /download/i });
    expect(downloads.some((link) => link.getAttribute("href")?.endsWith("Aseprite-Installer-macOS-Universal.dmg"))).toBe(true);
    expect(screen.getByRole("link", { name: /browse the code/i })).toHaveAttribute(
      "href",
      "https://github.com/fmhun/aseprite-installer",
    );
    expect(screen.getByText(/Windows/)).toHaveTextContent("planned");
    expect(screen.getByText(/Linux/)).toHaveTextContent("planned");
  });

  it("advances the walkthrough automatically", () => {
    vi.useFakeTimers();
    render(<ProductDemo />);
    const demo = screen.getByRole("figure");
    expect(demo).toHaveAttribute("data-phase", "status");

    act(() => vi.advanceTimersByTime(2_500));
    expect(demo).toHaveAttribute("data-phase", "release");
  });

  it("holds the final frame when reduced motion is enabled", () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      writable: true,
      value: matchMedia(true),
    });
    render(<ProductDemo />);
    expect(screen.getByRole("figure")).toHaveAttribute("data-phase", "complete");
    expect(screen.getByRole("figure")).toHaveAttribute("data-reduced-motion", "true");
  });
});
