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
      /Install Aseprite\s*from source\./,
    );
    expect(screen.getByText("AND FOR FREE")).toBeInTheDocument();
    expect(screen.getByText(/Aseprite Installer is a free, MIT-licensed macOS utility/)).toBeInTheDocument();
    const downloads = screen.getAllByRole("link", { name: /download/i });
    expect(downloads.some((link) => link.getAttribute("href")?.endsWith("Aseprite-Installer-macOS-Universal.dmg"))).toBe(true);
    expect(screen.getByRole("link", { name: /browse the code/i })).toHaveAttribute(
      "href",
      "https://github.com/fmhun/aseprite-installer",
    );
    expect(screen.getByRole("link", { name: "FAQ" })).toHaveAttribute("href", "#faq");
    expect(screen.queryByText("One legal check")).not.toBeInTheDocument();
    expect(screen.queryByText("LIVE WALKTHROUGH")).not.toBeInTheDocument();
    expect(screen.queryByText("BUILD REQUIREMENTS")).not.toBeInTheDocument();
    expect(screen.queryByText(/Windows/)).not.toBeInTheDocument();
    expect(screen.queryByText(/No telemetry/)).not.toBeInTheDocument();
    expect(screen.getByText(/~6 GB free/)).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Get ready to build" })).toBeInTheDocument();
    expect(screen.getByText(/Let the installer set up CMake and Ninja through Homebrew/)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Check your Mac" })).not.toBeInTheDocument();
  });

  it("places Install before How it works in the page and navigation", () => {
    render(<App />);

    const sectionIds = [...document.querySelectorAll("main > section")].map((section) => section.id);
    expect(sectionIds.indexOf("install")).toBeLessThan(sectionIds.indexOf("how-it-works"));

    const navLabels = [...document.querySelectorAll(".site-nav a")].map((link) => link.textContent?.trim());
    expect(navLabels.indexOf("Install")).toBeLessThan(navLabels.indexOf("How it works"));
  });

  it("advances the walkthrough automatically", () => {
    vi.useFakeTimers();
    render(<ProductDemo />);
    const demo = screen.getByRole("figure");
    expect(demo).toHaveAttribute("data-phase", "status");

    act(() => vi.advanceTimersByTime(2_500));
    expect(demo).toHaveAttribute("data-phase", "release");

    act(() => vi.advanceTimersByTime(5_500));
    expect(demo).toHaveAttribute("data-phase", "build");
    expect(screen.queryByText("One legal check")).not.toBeInTheDocument();
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
