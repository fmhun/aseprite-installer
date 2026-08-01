import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import indexHtml from "../index.html?raw";
import App, { ProductDemo } from "./App";

const RELEASES_LATEST_URL =
  "https://github.com/fmhun/aseprite-installer/releases/latest";
const ASSET_URL = `${RELEASES_LATEST_URL}/download`;
const originalVisibilityState = Object.getOwnPropertyDescriptor(
  document,
  "visibilityState",
);

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
    vi.unstubAllGlobals();
    if (originalVisibilityState) {
      Object.defineProperty(document, "visibilityState", originalVisibilityState);
    } else {
      Reflect.deleteProperty(document, "visibilityState");
    }
  });

  it("presents exact cross-platform release downloads", () => {
    render(<App />);

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      /Install Aseprite\s*from source\./,
    );
    expect(screen.getByText("AND FOR FREE")).toBeInTheDocument();
    expect(
      screen.getByText(/Aseprite Installer verifies official source/),
    ).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Download" })).toHaveAttribute(
      "href",
      "#install",
    );
    expect(
      screen.getByRole("link", { name: /choose your platform/i }),
    ).toHaveAttribute("href", "#install");
    expect(screen.getByRole("link", { name: /browse the code/i })).toHaveAttribute(
      "href",
      "https://github.com/fmhun/aseprite-installer",
    );
    expect(screen.getByRole("link", { name: "FAQ" })).toHaveAttribute("href", "#faq");
    expect(screen.queryByText("One legal check")).not.toBeInTheDocument();
    expect(screen.queryByText("LIVE WALKTHROUGH")).not.toBeInTheDocument();
    expect(screen.queryByText("BUILD REQUIREMENTS")).not.toBeInTheDocument();
    expect(screen.queryByText(/No telemetry/)).not.toBeInTheDocument();
    expect(screen.getAllByText(/about 6 GB/i)).toHaveLength(3);
    expect(screen.getByText(/Aseprite Installer checks every requirement/)).toBeInTheDocument();
    expect(screen.getByText(/System-level changes always remain under your control/)).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Prepare your system" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Check your Mac" })).not.toBeInTheDocument();
    expect(screen.getByText("A free tool developed for the Aseprite community.")).toBeInTheDocument();
    expect(screen.getByText(/No account, token, analytics, or hidden service\./)).toBeInTheDocument();
    expect(screen.queryByText(/hidden build service/)).not.toBeInTheDocument();

    expect(document.querySelectorAll('[role="tabpanel"]')).toHaveLength(3);
    expect(screen.getByText("Apple Silicon DMG").closest("a")).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-macOS-arm64.dmg`,
    );
    expect(screen.getByText("Intel DMG").closest("a")).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-macOS-x64.dmg`,
    );
    expect(screen.getByText("Apple Silicon app archive").closest("a")).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-macOS-arm64.app.zip`,
    );
    expect(screen.getByText("Intel app archive").closest("a")).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-macOS-x64.app.zip`,
    );
    expect(screen.getByText("NSIS installer").closest("a")).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-Windows-x64-setup.exe`,
    );
    expect(screen.getByText("MSI package").closest("a")).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-Windows-x64.msi`,
    );
    expect(screen.getByText("AppImage").closest("a")).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-Linux-x86_64.AppImage`,
    );
    expect(screen.getByText("deb package").closest("a")).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-Linux-x86_64.deb`,
    );
    expect(screen.getByText("rpm package").closest("a")).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-Linux-x86_64.rpm`,
    );
    expect(screen.getByRole("link", { name: /SHA256SUMS/i })).toHaveAttribute(
      "href",
      `${ASSET_URL}/SHA256SUMS`,
    );
    expect(screen.getByText(/existing safe Homebrew installation/)).toBeInTheDocument();
    expect(screen.getByText(/never launches Visual Studio Installer/)).toBeInTheDocument();
    expect(screen.getByText(/apt, dnf, pacman, or zypper/)).toBeInTheDocument();
    expect(screen.getByText(/Control-click the app/)).toBeInTheDocument();
    expect(screen.getByText(/SmartScreen may warn/)).toBeInTheDocument();
    expect(screen.getByText(/may also need/)).toHaveTextContent("chmod +x");
    expect(screen.getByText(/open the DMG/)).toBeInTheDocument();
    expect(screen.getByText(/run the current-user installer/)).toBeInTheDocument();
    expect(screen.getByText(/run the AppImage/)).toBeInTheDocument();
    expect(document.body).not.toHaveTextContent(/planned|universal dmg/i);
    expect(document.querySelector(".site-package-board")).not.toBeInTheDocument();
    expect(document.querySelector(".site-platforms")).not.toBeInTheDocument();
  });

  it("uses an accessible deterministic platform tab interface", () => {
    render(<App />);

    const [macosTab, windowsTab, linuxTab] = screen.getAllByRole("tab");
    const panels = [...document.querySelectorAll<HTMLElement>('[role="tabpanel"]')];

    expect(macosTab).toHaveAttribute("aria-selected", "true");
    expect(windowsTab).toHaveAttribute("aria-selected", "false");
    expect(panels[0]).not.toHaveAttribute("hidden");
    expect(panels[1]).toHaveAttribute("hidden");
    expect(panels[2]).toHaveAttribute("hidden");

    fireEvent.keyDown(macosTab, { key: "ArrowRight" });
    expect(windowsTab).toHaveFocus();
    expect(windowsTab).toHaveAttribute("aria-selected", "true");
    expect(panels[1]).not.toHaveAttribute("hidden");

    fireEvent.keyDown(windowsTab, { key: "ArrowLeft" });
    expect(macosTab).toHaveFocus();
    expect(macosTab).toHaveAttribute("aria-selected", "true");

    fireEvent.click(linuxTab);
    expect(linuxTab).toHaveAttribute("aria-selected", "true");
    expect(panels[2]).not.toHaveAttribute("hidden");

    fireEvent.keyDown(linuxTab, { key: "Home" });
    expect(macosTab).toHaveFocus();
    expect(macosTab).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(macosTab, { key: "End" });
    expect(linuxTab).toHaveFocus();
    fireEvent.keyDown(linuxTab, { key: "ArrowRight" });
    expect(macosTab).toHaveFocus();
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
      "Aseprite Installer for macOS, Windows &amp; Linux",
    );
    expect(indexHtml).toContain(
      "builds your personal copy locally on macOS, Windows 11, or Linux",
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

  it("pauses the walkthrough while it is outside the viewport", () => {
    vi.useFakeTimers();
    let intersectionCallback: IntersectionObserverCallback | undefined;

    class IntersectionObserverMock implements IntersectionObserver {
      readonly root = null;
      readonly rootMargin = "0px";
      readonly thresholds = [0.2];

      constructor(callback: IntersectionObserverCallback) {
        intersectionCallback = callback;
      }

      disconnect() {}
      observe() {}
      takeRecords() { return []; }
      unobserve() {}
    }

    vi.stubGlobal("IntersectionObserver", IntersectionObserverMock);
    render(<ProductDemo />);
    const demo = screen.getByRole("figure");

    expect(demo).toHaveAttribute("data-playing", "false");
    act(() => {
      intersectionCallback?.(
        [{ isIntersecting: true } as IntersectionObserverEntry],
        {} as IntersectionObserver,
      );
    });
    expect(demo).toHaveAttribute("data-playing", "true");

    act(() => vi.advanceTimersByTime(2_500));
    expect(demo).toHaveAttribute("data-phase", "release");

    act(() => {
      intersectionCallback?.(
        [{ isIntersecting: false } as IntersectionObserverEntry],
        {} as IntersectionObserver,
      );
    });
    expect(demo).toHaveAttribute("data-playing", "false");

    act(() => vi.advanceTimersByTime(6_000));
    expect(demo).toHaveAttribute("data-phase", "release");
  });

  it("pauses the walkthrough while the page is hidden", () => {
    vi.useFakeTimers();
    let visibilityState: DocumentVisibilityState = "visible";
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => visibilityState,
    });

    render(<ProductDemo />);
    const demo = screen.getByRole("figure");
    expect(demo).toHaveAttribute("data-playing", "true");

    act(() => vi.advanceTimersByTime(2_500));
    expect(demo).toHaveAttribute("data-phase", "release");

    visibilityState = "hidden";
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    expect(demo).toHaveAttribute("data-playing", "false");

    act(() => vi.advanceTimersByTime(6_000));
    expect(demo).toHaveAttribute("data-phase", "release");
  });
});
