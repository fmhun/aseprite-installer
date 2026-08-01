import { act, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import indexHtml from "../index.html?raw";
import App, { ProductDemo } from "./App";

const RELEASES_LATEST_URL =
  "https://github.com/fmhun/aseprite-installer/releases/latest";

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

  it("presents supported installers without guessing release asset names", () => {
    render(<App />);

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      /Install Aseprite\s*from source\./,
    );
    expect(screen.getByText("AND FOR FREE")).toBeInTheDocument();
    expect(
      screen.getByText(/Aseprite Installer is a free, MIT-licensed desktop utility/),
    ).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Downloads" })).toHaveAttribute(
      "href",
      RELEASES_LATEST_URL,
    );
    expect(
      screen.getByRole("link", { name: /choose your installer/i }),
    ).toHaveAttribute("href", RELEASES_LATEST_URL);
    expect(
      screen.getByRole("link", { name: /view latest release/i }),
    ).toHaveAttribute("href", RELEASES_LATEST_URL);
    expect(screen.getByRole("link", { name: /browse the code/i })).toHaveAttribute(
      "href",
      "https://github.com/fmhun/aseprite-installer",
    );
    expect(screen.getByRole("link", { name: "FAQ" })).toHaveAttribute("href", "#faq");
    expect(screen.queryByText("One legal check")).not.toBeInTheDocument();
    expect(screen.queryByText("LIVE WALKTHROUGH")).not.toBeInTheDocument();
    expect(screen.queryByText("BUILD REQUIREMENTS")).not.toBeInTheDocument();
    expect(screen.queryByText(/No telemetry/)).not.toBeInTheDocument();
    expect(screen.getByText(/~6 GB free/)).toBeInTheDocument();
    expect(screen.getByText(/Aseprite Installer checks your setup for you/)).toBeInTheDocument();
    expect(screen.getByText(/use the built-in platform guides to resolve anything missing/)).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Get ready to build" })).toBeInTheDocument();
    expect(screen.getByText(/Check the native compiler and SDK/)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Check your Mac" })).not.toBeInTheDocument();
    expect(screen.getByText("A free tool developed for the Aseprite community.")).toBeInTheDocument();
    expect(screen.getByText(/No account, token, analytics, or hidden service\./)).toBeInTheDocument();
    expect(screen.queryByText(/hidden build service/)).not.toBeInTheDocument();

    const packages = within(
      screen.getByLabelText("Available installer packages by platform"),
    );
    expect(packages.getByText("Apple Silicon + Intel")).toBeInTheDocument();
    expect(packages.getByText("NSIS .exe · MSI")).toBeInTheDocument();
    expect(packages.getByText("AppImage · .deb · .rpm")).toBeInTheDocument();

    const availability = within(screen.getByLabelText("Platform availability"));
    expect(availability.getByText(/macOS/)).toHaveTextContent("available");
    expect(availability.getByText(/Windows/)).toHaveTextContent("available");
    expect(availability.getByText(/Linux/)).toHaveTextContent("available");
    expect(document.body).not.toHaveTextContent(/planned|universal dmg/i);
    expect(document.body.innerHTML).not.toContain("/releases/latest/download/");
  });

  it("places Install before How it works in the page and navigation", () => {
    render(<App />);

    const sectionIds = [...document.querySelectorAll("main > section")].map((section) => section.id);
    expect(sectionIds.indexOf("install")).toBeLessThan(sectionIds.indexOf("how-it-works"));

    const navLabels = [...document.querySelectorAll(".site-nav a")].map((link) => link.textContent?.trim());
    expect(navLabels.indexOf("Install")).toBeLessThan(navLabels.indexOf("How it works"));
  });

  it("publishes cross-platform metadata", () => {
    expect(indexHtml).toContain(
      "Build on macOS, Windows and Linux",
    );
    expect(indexHtml).toContain(
      "verified personal Aseprite source builds on macOS, Windows 11, and Linux",
    );
    expect(indexHtml).not.toMatch(/macOS utility|Universal\.dmg|\/releases\/latest\/download\//);
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
