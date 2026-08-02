import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import indexHtml from "../index.html?raw";
import App, { ProductDemo } from "./App";
import type { NavigatorLike } from "./platformDetection";
import {
  resetPlatformSimulation,
  simulatePlatform,
} from "./platformSimulation";

const RELEASES_LATEST_URL =
  "https://github.com/fmhun/aseprite-installer/releases/latest";
const ASSET_URL = `${RELEASES_LATEST_URL}/download`;
const APT_INSTALL_SCRIPT_URL =
  "https://fmhun.github.io/aseprite-installer/install-apt.sh";
const LINUX_DOWNLOAD_DIRECTORY_COMMAND =
  'ASEPRITE_DOWNLOAD_DIR="$(xdg-user-dir DOWNLOAD 2>/dev/null)"; ASEPRITE_DOWNLOAD_DIR="${ASEPRITE_DOWNLOAD_DIR:-$HOME/Downloads}"';
const originalClipboardDescriptor = Object.getOwnPropertyDescriptor(
  window.navigator,
  "clipboard",
);
const originalExecCommandDescriptor = Object.getOwnPropertyDescriptor(
  document,
  "execCommand",
);
const originalVisibilityState = Object.getOwnPropertyDescriptor(
  document,
  "visibilityState",
);
const navigatorKeys = ["userAgent", "platform", "maxTouchPoints", "userAgentData"] as const;
const originalNavigatorDescriptors = new Map(
  navigatorKeys.map((key) => [key, Object.getOwnPropertyDescriptor(window.navigator, key)]),
);

function mockNavigator(values: NavigatorLike) {
  for (const key of navigatorKeys) {
    if (!(key in values)) continue;
    Object.defineProperty(window.navigator, key, {
      configurable: true,
      value: values[key],
    });
  }
}

function restoreNavigator() {
  for (const key of navigatorKeys) {
    const descriptor = originalNavigatorDescriptors.get(key);
    if (descriptor) {
      Object.defineProperty(window.navigator, key, descriptor);
    } else {
      Reflect.deleteProperty(window.navigator, key);
    }
  }
}

function expectedLinuxCommand(asset: string, installCommand: string) {
  const assetUrl = `${ASSET_URL}/${asset}`;
  const curlDownloads = `curl -fL --retry 3 -o "${asset}" "${assetUrl}" && curl -fL --retry 3 -o SHA256SUMS "${ASSET_URL}/SHA256SUMS"`;
  const wgetDownloads = `wget -O "${asset}" "${assetUrl}" && wget -O SHA256SUMS "${ASSET_URL}/SHA256SUMS"`;

  return `${LINUX_DOWNLOAD_DIRECTORY_COMMAND}; mkdir -p "$ASEPRITE_DOWNLOAD_DIR" && cd "$ASEPRITE_DOWNLOAD_DIR" && ( (${curlDownloads}) || (${wgetDownloads}) ) && grep -F "  ${asset}" SHA256SUMS > "${asset}.sha256" && sha256sum --check "${asset}.sha256" && rm -f "${asset}.sha256" && ${installCommand}`;
}

const appImageCommand = expectedLinuxCommand(
  "Aseprite-Installer-Linux-x86_64.AppImage",
  'chmod u+x "Aseprite-Installer-Linux-x86_64.AppImage" && "./Aseprite-Installer-Linux-x86_64.AppImage"',
);
const debCommand =
  `curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 ${APT_INSTALL_SCRIPT_URL} | sh`;
const rpmCommand = expectedLinuxCommand(
  "Aseprite-Installer-Linux-x86_64.rpm",
  'sudo dnf install "./Aseprite-Installer-Linux-x86_64.rpm"',
);
const zypperCommand = expectedLinuxCommand(
  "Aseprite-Installer-Linux-x86_64.rpm",
  'sudo zypper install "./Aseprite-Installer-Linux-x86_64.rpm"',
);

function mockClipboard(
  writeText: ((value: string) => Promise<void>) | undefined,
) {
  Object.defineProperty(window.navigator, "clipboard", {
    configurable: true,
    value: writeText ? { writeText } : undefined,
  });
}

function mockExecCommand(copy: (commandId: string) => boolean) {
  Object.defineProperty(document, "execCommand", {
    configurable: true,
    value: copy,
  });
}

function restoreClipboard() {
  if (originalClipboardDescriptor) {
    Object.defineProperty(
      window.navigator,
      "clipboard",
      originalClipboardDescriptor,
    );
  } else {
    Reflect.deleteProperty(window.navigator, "clipboard");
  }

  if (originalExecCommandDescriptor) {
    Object.defineProperty(
      document,
      "execCommand",
      originalExecCommandDescriptor,
    );
  } else {
    Reflect.deleteProperty(document, "execCommand");
  }
}

async function renderLinuxLanding() {
  mockNavigator({
    userAgent: "Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/141.0",
    platform: "Linux x86_64",
    maxTouchPoints: 0,
  });
  render(<App />);

  const linuxTab = screen.getByRole("tab", { name: "Linux" });
  await waitFor(() => expect(linuxTab).toHaveAttribute("aria-selected", "true"));
  return linuxTab;
}

function linuxViewer() {
  const viewer = document.querySelector<HTMLElement>(".site-quick-install");
  if (!viewer) throw new Error("Linux quick-install viewer was not rendered");
  return within(viewer);
}

function renderedLinuxCommand() {
  return document.querySelector(".site-command-line code")?.textContent;
}

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
    restoreClipboard();
    restoreNavigator();
    if (originalVisibilityState) {
      Object.defineProperty(document, "visibilityState", originalVisibilityState);
    } else {
      Reflect.deleteProperty(document, "visibilityState");
    }
  });

  it("presents exact cross-platform release downloads", () => {
    mockNavigator({ userAgent: "", platform: "", maxTouchPoints: 0 });
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
    expect(
      screen.getByText("AppImage", { selector: ".site-package-link strong" }).closest("a"),
    ).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-Linux-x86_64.AppImage`,
    );
    expect(screen.getByText("Direct .deb").closest("a")).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-Linux-x86_64.deb`,
    );
    expect(screen.getByText("Direct .rpm").closest("a")).toHaveAttribute(
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
    expect(
      document.querySelector("#platform-linux-panel .site-platform-warning"),
    ).toHaveTextContent(
      "APT verifies signed repository metadata and package hashes automatically",
    );
    expect(screen.getByText(/open the DMG/)).toBeInTheDocument();
    expect(screen.getByText(/run the current-user installer/)).toBeInTheDocument();
    expect(
      document.querySelector("#platform-linux-panel .site-platform-install"),
    ).toHaveTextContent(
      "The Debian/Ubuntu bootstrap configures APT once; future updates arrive with normal system updates.",
    );
    expect(document.body).not.toHaveTextContent(/planned|universal dmg/i);
    expect(document.querySelector(".site-package-board")).not.toBeInTheDocument();
    expect(document.querySelector(".site-platforms")).not.toBeInTheDocument();
  });

  it("uses an accessible deterministic platform tab interface", () => {
    mockNavigator({
      userAgent:
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.6 Safari/605.1.15",
      platform: "MacIntel",
      maxTouchPoints: 0,
    });
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

  describe("Linux quick install viewer", () => {
    it("starts on the verified AppImage recipe with an accessible pressed state", async () => {
      await renderLinuxLanding();
      const viewer = linuxViewer();

      expect(viewer.getByRole("button", { name: "AppImage" })).toHaveAttribute(
        "aria-pressed",
        "true",
      );
      expect(
        viewer.getByRole("button", { name: "Debian / Ubuntu" }),
      ).toHaveAttribute("aria-pressed", "false");
      expect(
        viewer.getByRole("button", { name: "Fedora / RHEL" }),
      ).toHaveAttribute("aria-pressed", "false");
      expect(viewer.getByRole("button", { name: "openSUSE" })).toHaveAttribute(
        "aria-pressed",
        "false",
      );
      expect(renderedLinuxCommand()).toBe(appImageCommand);
    });

    it("switches Debian, Fedora, and openSUSE recipes and keeps direct packages distinct", async () => {
      await renderLinuxLanding();
      const viewer = linuxViewer();
      const debRecipe = viewer.getByRole("button", { name: "Debian / Ubuntu" });
      const rpmRecipe = viewer.getByRole("button", { name: "Fedora / RHEL" });
      const zypperRecipe = viewer.getByRole("button", { name: "openSUSE" });

      fireEvent.click(debRecipe);
      expect(debRecipe).toHaveAttribute("aria-pressed", "true");
      expect(renderedLinuxCommand()).toBe(debCommand);
      expect(viewer.getByRole("link", { name: /Inspect the bootstrap/i })).toHaveAttribute(
        "href",
        "https://github.com/fmhun/aseprite-installer/blob/main/site/public/install-apt.sh",
      );
      expect(viewer.getByText(/future updates/i)).toBeInTheDocument();
      expect(screen.getByRole("link", { name: "bootstrap script" })).toHaveAttribute(
        "href",
        APT_INSTALL_SCRIPT_URL,
      );
      expect(screen.getByRole("link", { name: "public key" })).toHaveAttribute(
        "href",
        "https://fmhun.github.io/aseprite-installer/apt/aseprite-installer-archive-keyring.asc",
      );
      expect(screen.getByRole("link", { name: "source definition" })).toHaveAttribute(
        "href",
        "https://fmhun.github.io/aseprite-installer/apt/aseprite-installer.sources",
      );
      expect(screen.getByRole("link", { name: "origin policy" })).toHaveAttribute(
        "href",
        "https://fmhun.github.io/aseprite-installer/apt/aseprite-installer.pref",
      );

      fireEvent.click(rpmRecipe);
      expect(rpmRecipe).toHaveAttribute("aria-pressed", "true");
      expect(renderedLinuxCommand()).toBe(rpmCommand);

      fireEvent.click(zypperRecipe);
      expect(zypperRecipe).toHaveAttribute("aria-pressed", "true");
      expect(renderedLinuxCommand()).toBe(zypperCommand);

      fireEvent.click(zypperRecipe);
      const aptSetupLink = screen.getByText("Signed APT setup").closest("a");
      expect(aptSetupLink).toHaveAttribute("href", "#linux-quick-install-title");
      aptSetupLink!.addEventListener("click", (event) => event.preventDefault(), {
        once: true,
      });
      fireEvent.click(aptSetupLink!);
      expect(debRecipe).toHaveAttribute("aria-pressed", "true");
      expect(renderedLinuxCommand()).toBe(debCommand);

      fireEvent.click(zypperRecipe);
      const directDebLink = screen.getByText("Direct .deb").closest("a");
      expect(directDebLink).toHaveAttribute("href", `${ASSET_URL}/Aseprite-Installer-Linux-x86_64.deb`);
      directDebLink!.addEventListener("click", (event) => event.preventDefault(), {
        once: true,
      });
      fireEvent.click(directDebLink!);
      expect(zypperRecipe).toHaveAttribute("aria-pressed", "true");
      expect(renderedLinuxCommand()).toBe(zypperCommand);
    });

    it("copies the exact selected command with the Clipboard API", async () => {
      const writeText = vi.fn<(value: string) => Promise<void>>().mockResolvedValue();
      mockClipboard(writeText);
      await renderLinuxLanding();
      const viewer = linuxViewer();
      const copyButton = viewer.getByRole("button", {
        name: "Copy AppImage install command",
      });

      fireEvent.click(copyButton);

      await waitFor(() => expect(writeText).toHaveBeenCalledWith(appImageCommand));
      expect(writeText).toHaveBeenCalledTimes(1);
      expect(copyButton).toHaveTextContent("Copied!");
      expect(viewer.getByRole("status")).toHaveTextContent(
        "AppImage install command copied to clipboard.",
      );
    });

    it("falls back to execCommand and restores focus to the copy button", async () => {
      let selectedValue = "";
      const execCommand = vi.fn((commandId: string) => {
        selectedValue = document.querySelector<HTMLTextAreaElement>("textarea")?.value ?? "";
        return commandId === "copy";
      });
      mockClipboard(undefined);
      mockExecCommand(execCommand);
      await renderLinuxLanding();
      const viewer = linuxViewer();
      const copyButton = viewer.getByRole("button", {
        name: "Copy AppImage install command",
      });
      copyButton.focus();

      fireEvent.click(copyButton);

      await waitFor(() => expect(execCommand).toHaveBeenCalledWith("copy"));
      expect(selectedValue).toBe(appImageCommand);
      expect(copyButton).toHaveFocus();
      expect(copyButton).toHaveTextContent("Copied!");
      expect(document.querySelector("textarea")).not.toBeInTheDocument();
    });

    it("reports a total clipboard failure without claiming success", async () => {
      const writeText = vi
        .fn<(value: string) => Promise<void>>()
        .mockRejectedValue(new Error("Clipboard denied"));
      const execCommand = vi.fn(() => false);
      mockClipboard(writeText);
      mockExecCommand(execCommand);
      await renderLinuxLanding();
      const viewer = linuxViewer();
      const copyButton = viewer.getByRole("button", {
        name: "Copy AppImage install command",
      });
      copyButton.focus();

      fireEvent.click(copyButton);

      await waitFor(() => expect(copyButton).toHaveTextContent("Copy failed"));
      expect(writeText).toHaveBeenCalledWith(appImageCommand);
      expect(execCommand).toHaveBeenCalledWith("copy");
      expect(copyButton).toHaveFocus();
      expect(copyButton).not.toHaveClass("is-copied");
      expect(viewer.getByRole("status")).toHaveTextContent(
        "Automatic copy failed. Select the command and copy it manually.",
      );
    });

    it("ignores an obsolete async copy result after the recipe changes", async () => {
      let resolveCopy: (() => void) | undefined;
      const pendingCopy = new Promise<void>((resolve) => {
        resolveCopy = resolve;
      });
      const writeText = vi.fn(() => pendingCopy);
      mockClipboard(writeText);
      await renderLinuxLanding();
      const viewer = linuxViewer();

      fireEvent.click(
        viewer.getByRole("button", { name: "Copy AppImage install command" }),
      );
      fireEvent.click(viewer.getByRole("button", { name: "Debian / Ubuntu" }));
      expect(renderedLinuxCommand()).toBe(debCommand);

      await act(async () => {
        resolveCopy?.();
        await pendingCopy;
      });

      expect(writeText).toHaveBeenCalledWith(appImageCommand);
      expect(
        viewer.getByRole("button", { name: "Copy Debian / Ubuntu install command" }),
      ).toHaveTextContent("Copy");
      expect(viewer.getByRole("status")).toBeEmptyDOMElement();
    });

    it("resets copy feedback after its timeout", async () => {
      const writeText = vi.fn<(value: string) => Promise<void>>().mockResolvedValue();
      mockClipboard(writeText);
      await renderLinuxLanding();
      const viewer = linuxViewer();
      const copyButton = viewer.getByRole("button", {
        name: "Copy AppImage install command",
      });
      vi.useFakeTimers();

      fireEvent.click(copyButton);
      await act(async () => {
        await Promise.resolve();
      });
      expect(copyButton).toHaveTextContent("Copied!");

      act(() => vi.advanceTimersByTime(2_000));

      expect(copyButton).toHaveTextContent("Copy");
      expect(viewer.getByRole("status")).toBeEmptyDOMElement();
    });
  });

  describe("Chrome console platform simulation", () => {
    beforeEach(() => {
      resetPlatformSimulation();
    });

    afterEach(() => {
      act(() => {
        resetPlatformSimulation();
      });
    });

    it.each([
      [
        "macos-arm64",
        "macOS",
        "macos-arm64",
        "Download for Apple Silicon",
        `${ASSET_URL}/Aseprite-Installer-macOS-arm64.dmg`,
        "Apple Silicon detected · Apple Silicon DMG recommended",
      ],
      [
        "macos-x64",
        "macOS",
        "macos-x64",
        "Download for Intel Mac",
        `${ASSET_URL}/Aseprite-Installer-macOS-x64.dmg`,
        "Intel Mac detected · Intel DMG recommended",
      ],
      [
        "windows-x64",
        "Windows",
        "windows-x64",
        "Download for Windows",
        `${ASSET_URL}/Aseprite-Installer-Windows-x64-setup.exe`,
        "Windows x64 detected · NSIS installer recommended",
      ],
      [
        "linux-x64",
        "Linux",
        "linux-x64",
        "Download AppImage",
        `${ASSET_URL}/Aseprite-Installer-Linux-x86_64.AppImage`,
        "Linux x86_64 detected · AppImage recommended",
      ],
    ] as const)(
      "simulates the %s direct-download client without navigating",
      async (
        simulationId,
        tabName,
        downloadTarget,
        ctaName,
        expectedHref,
        expectedStatus,
      ) => {
        mockNavigator({ userAgent: "", platform: "", maxTouchPoints: 0 });
        render(<App />);

        act(() => {
          simulatePlatform(simulationId);
        });

        await waitFor(() => {
          expect(screen.getByRole("tab", { name: tabName })).toHaveAttribute(
            "aria-selected",
            "true",
          );
          expect(screen.getByRole("link", { name: new RegExp(ctaName, "i") })).toHaveAttribute(
            "href",
            expectedHref,
          );
        });

        expect(document.querySelector(".site-page")).toHaveAttribute(
          "data-platform-simulation",
          simulationId,
        );
        expect(document.querySelector(".site-detection-note")).toHaveTextContent(
          `Simulation · ${expectedStatus} · Chrome console override`,
        );
        expect(
          screen.getByRole("link", { name: new RegExp(ctaName, "i") }),
        ).toHaveAttribute("data-download-target", downloadTarget);
      },
    );

    it("clears a manual platform choice when a new simulation is requested", async () => {
      mockNavigator({ userAgent: "", platform: "", maxTouchPoints: 0 });
      render(<App />);

      act(() => {
        simulatePlatform("macos-arm64");
      });
      await waitFor(() => {
        expect(screen.getByRole("link", { name: /Download for Apple Silicon/i })).toBeInTheDocument();
      });

      fireEvent.click(screen.getByRole("tab", { name: "Windows" }));
      expect(document.querySelector(".site-detection-note")).toHaveTextContent(
        "Windows selected manually · automatic selection paused",
      );
      expect(document.querySelector(".site-page")).toHaveAttribute(
        "data-platform-simulation",
        "macos-arm64",
      );
      expect(document.querySelector(".site-page")).toHaveAttribute(
        "data-effective-platform",
        "windows",
      );

      act(() => {
        simulatePlatform("linux-x64");
      });

      await waitFor(() => {
        expect(screen.getByRole("tab", { name: "Linux" })).toHaveAttribute(
          "aria-selected",
          "true",
        );
        expect(screen.getByRole("link", { name: /Download AppImage/i })).toHaveAttribute(
          "data-download-target",
          "linux-x64",
        );
      });
      expect(document.querySelector(".site-detection-note")).toHaveTextContent(
        "Simulation · Linux x86_64 detected · AppImage recommended · Chrome console override",
      );
      expect(document.querySelector(".site-detection-note")).not.toHaveTextContent(
        "selected manually",
      );
    });

    it("returns to the latest real detector result after reset", async () => {
      mockNavigator({
        userAgent:
          "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140 Safari/537.36",
        platform: "Win32",
        maxTouchPoints: 0,
        userAgentData: {
          platform: "Windows",
          mobile: false,
          getHighEntropyValues: vi.fn().mockResolvedValue({
            architecture: "x86",
            bitness: "64",
            platformVersion: "13.0.0",
          }),
        },
      });
      render(<App />);

      await screen.findByRole("link", { name: /Download for Windows/i });
      act(() => {
        simulatePlatform("macos-arm64");
      });
      await screen.findByRole("link", { name: /Download for Apple Silicon/i });

      act(() => {
        resetPlatformSimulation();
      });

      await waitFor(() => {
        expect(screen.getByRole("link", { name: /Download for Windows/i })).toHaveAttribute(
          "href",
          `${ASSET_URL}/Aseprite-Installer-Windows-x64-setup.exe`,
        );
      });
      expect(document.querySelector(".site-page")).toHaveAttribute(
        "data-platform-simulation",
        "none",
      );
      expect(document.querySelector(".site-detection-note")).toHaveTextContent(
        "Windows x64 detected · NSIS installer recommended",
      );
      expect(document.querySelector(".site-detection-note")).not.toHaveTextContent(
        "Simulation",
      );
    });

    it("keeps a simulation authoritative when async client hints resolve later", async () => {
      type WindowsHints = {
        architecture: string;
        bitness: string;
        platformVersion: string;
      };
      let resolveHints: (value: WindowsHints) => void = () => undefined;
      const highEntropyValues = new Promise<WindowsHints>((resolve) => {
        resolveHints = resolve;
      });
      mockNavigator({
        userAgent:
          "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140 Safari/537.36",
        platform: "Win32",
        maxTouchPoints: 0,
        userAgentData: {
          platform: "Windows",
          mobile: false,
          getHighEntropyValues: vi.fn().mockReturnValue(highEntropyValues),
        },
      });
      render(<App />);

      act(() => {
        simulatePlatform("linux-x64");
      });
      await screen.findByRole("link", { name: /Download AppImage/i });

      await act(async () => {
        resolveHints({
          architecture: "x86",
          bitness: "64",
          platformVersion: "13.0.0",
        });
        await highEntropyValues;
      });

      expect(screen.getByRole("tab", { name: "Linux" })).toHaveAttribute(
        "aria-selected",
        "true",
      );
      expect(screen.getByRole("link", { name: /Download AppImage/i })).toHaveAttribute(
        "data-download-target",
        "linux-x64",
      );
      expect(document.querySelector(".site-page")).toHaveAttribute(
        "data-platform-simulation",
        "linux-x64",
      );
    });

    it("does not change a focused CTA until focus leaves it", async () => {
      mockNavigator({ userAgent: "", platform: "", maxTouchPoints: 0 });
      render(<App />);
      const chooser = screen.getByRole("link", { name: /Choose your platform/i });
      chooser.focus();

      act(() => {
        simulatePlatform("macos-arm64");
      });

      expect(chooser).toHaveFocus();
      expect(chooser).toHaveAttribute("href", "#install");
      expect(chooser).toHaveAttribute("data-download-target", "picker");
      expect(document.querySelector(".site-page")).toHaveAttribute(
        "data-platform-simulation",
        "macos-arm64",
      );

      fireEvent.blur(chooser);
      await waitFor(() => {
        expect(screen.getByRole("link", { name: /Download for Apple Silicon/i })).toHaveAttribute(
          "data-download-target",
          "macos-arm64",
        );
      });
    });

    it("keeps a focused CTA stable while a simulation clears a manual choice", async () => {
      mockNavigator({ userAgent: "", platform: "", maxTouchPoints: 0 });
      render(<App />);
      fireEvent.click(screen.getByRole("tab", { name: "Windows" }));
      const manualChooser = await screen.findByRole("link", {
        name: /Choose a Windows package/i,
      });
      manualChooser.focus();

      act(() => {
        simulatePlatform("linux-x64");
      });

      expect(manualChooser).toHaveFocus();
      expect(manualChooser).toHaveAttribute("href", "#install");
      expect(manualChooser).toHaveAttribute("data-download-target", "picker");

      fireEvent.blur(manualChooser);
      await waitFor(() => {
        expect(screen.getByRole("link", { name: /Download AppImage/i })).toHaveAttribute(
          "data-download-target",
          "linux-x64",
        );
      });
    });

    it("keeps the CTA stable during a pointer gesture and applies the latest simulation", async () => {
      mockNavigator({ userAgent: "", platform: "", maxTouchPoints: 0 });
      render(<App />);
      const chooser = screen.getByRole("link", { name: /Choose your platform/i });

      fireEvent.pointerDown(chooser);
      act(() => {
        simulatePlatform("windows-x64");
        simulatePlatform("linux-x64");
      });

      expect(chooser).toHaveAttribute("href", "#install");
      expect(chooser).toHaveAttribute("data-download-target", "picker");
      expect(document.querySelector(".site-page")).toHaveAttribute(
        "data-platform-simulation",
        "linux-x64",
      );

      fireEvent.pointerCancel(chooser);
      await waitFor(() => {
        expect(screen.getByRole("link", { name: /Download AppImage/i })).toHaveAttribute(
          "data-download-target",
          "linux-x64",
        );
      });
    });

    it("releases the CTA guard when the pointer is released outside the link", async () => {
      mockNavigator({ userAgent: "", platform: "", maxTouchPoints: 0 });
      render(<App />);
      const chooser = screen.getByRole("link", { name: /Choose your platform/i });

      fireEvent.pointerDown(chooser);
      act(() => {
        simulatePlatform("linux-x64");
      });

      expect(chooser).toHaveAttribute("href", "#install");
      expect(chooser).toHaveAttribute("data-download-target", "picker");

      fireEvent.pointerUp(window);

      await waitFor(() => {
        expect(screen.getByRole("link", { name: /Download AppImage/i })).toHaveAttribute(
          "data-download-target",
          "linux-x64",
        );
      });
    });
  });

  it("auto-selects Windows and exposes the verified one-click package", async () => {
    mockNavigator({
      userAgent:
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140 Safari/537.36",
      platform: "Win32",
      maxTouchPoints: 0,
      userAgentData: {
        platform: "Windows",
        mobile: false,
        getHighEntropyValues: vi.fn().mockResolvedValue({
          architecture: "x86",
          bitness: "64",
          platformVersion: "13.0.0",
        }),
      },
    });
    render(<App />);

    const windowsTab = screen.getByRole("tab", { name: "Windows" });
    await waitFor(() => expect(windowsTab).toHaveAttribute("aria-selected", "true"));
    const smartDownload = await screen.findByRole("link", { name: /Download for Windows/i });
    expect(smartDownload).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-Windows-x64-setup.exe`,
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "Windows x64 detected · NSIS installer recommended",
    );
    expect(document.activeElement).toBe(document.body);
  });

  it("uses client hints for a one-click Apple Silicon download", async () => {
    mockNavigator({
      userAgent:
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/140 Safari/537.36",
      platform: "MacIntel",
      maxTouchPoints: 0,
      userAgentData: {
        platform: "macOS",
        mobile: false,
        getHighEntropyValues: vi.fn().mockResolvedValue({
          architecture: "arm",
          bitness: "64",
          platformVersion: "15.2.0",
        }),
      },
    });
    render(<App />);

    const smartDownload = await screen.findByRole("link", {
      name: /Download for Apple Silicon/i,
    });
    expect(smartDownload).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-macOS-arm64.dmg`,
    );
  });

  it("recommends AppImage to a legacy Linux x86_64 browser", async () => {
    mockNavigator({
      userAgent: "Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/141.0",
      platform: "Linux x86_64",
      maxTouchPoints: 0,
    });
    render(<App />);

    expect(await screen.findByRole("link", { name: /Download AppImage/i })).toHaveAttribute(
      "href",
      `${ASSET_URL}/Aseprite-Installer-Linux-x86_64.AppImage`,
    );
    expect(screen.getByRole("tab", { name: "Linux" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("selects macOS without guessing Safari's CPU architecture", async () => {
    mockNavigator({
      userAgent:
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.6 Safari/605.1.15",
      platform: "MacIntel",
      maxTouchPoints: 0,
    });
    render(<App />);

    const chooser = await screen.findByRole("link", {
      name: /Choose Apple Silicon or Intel/i,
    });
    expect(chooser).toHaveAttribute("href", "#install");
    expect(screen.getByRole("status")).toHaveTextContent(
      "macOS detected · choose Apple Silicon or Intel",
    );
  });

  it("never offers a desktop package to iPadOS desktop mode", async () => {
    mockNavigator({
      userAgent:
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) AppleWebKit/605.1.15 Mobile/15E148 Safari/604.1",
      platform: "MacIntel",
      maxTouchPoints: 5,
    });
    render(<App />);

    const chooser = await screen.findByRole("link", { name: /Choose your platform/i });
    expect(chooser).toHaveAttribute("href", "#install");
    expect(screen.getByRole("status")).toHaveTextContent(
      "Mobile device detected · choose the target desktop computer",
    );
    expect(document.querySelector(".site-download-picker")).toHaveAttribute(
      "data-active-platform",
      "none",
    );
    expect(screen.getByText("Choose the desktop you want to install on.")).toBeVisible();
    for (const tab of screen.getAllByRole("tab")) {
      expect(tab).toHaveAttribute("aria-selected", "false");
    }
    for (const panel of document.querySelectorAll('[role="tabpanel"]')) {
      expect(panel).toHaveAttribute("hidden");
    }
  });

  it("keeps a manual tab choice when client hints resolve later", async () => {
    type WindowsHints = { architecture: string; bitness: string; platformVersion: string };
    let resolveHints: (value: WindowsHints) => void = () => undefined;
    const highEntropyValues = new Promise<WindowsHints>((resolve) => {
      resolveHints = resolve;
    });
    mockNavigator({
      userAgent:
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140 Safari/537.36",
      platform: "Win32",
      userAgentData: {
        platform: "Windows",
        mobile: false,
        getHighEntropyValues: vi.fn().mockReturnValue(highEntropyValues),
      },
    });
    render(<App />);

    const linuxTab = screen.getByRole("tab", { name: "Linux" });
    fireEvent.click(linuxTab);
    expect(linuxTab).toHaveAttribute("aria-selected", "true");

    await act(async () => {
      resolveHints({ architecture: "x86", bitness: "64", platformVersion: "13.0.0" });
      await highEntropyValues;
    });

    expect(linuxTab).toHaveAttribute("aria-selected", "true");
    expect(document.querySelector(".site-detection-note")).toHaveTextContent(
      "Linux selected manually · automatic selection paused",
    );
    expect(screen.getByRole("link", { name: /Choose a Linux package/i })).toHaveAttribute(
      "href",
      "#install",
    );
  });

  it("does not change a focused CTA into a download underneath the user", async () => {
    type WindowsHints = { architecture: string; bitness: string; platformVersion: string };
    let resolveHints: (value: WindowsHints) => void = () => undefined;
    const highEntropyValues = new Promise<WindowsHints>((resolve) => {
      resolveHints = resolve;
    });
    mockNavigator({
      userAgent:
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140 Safari/537.36",
      platform: "Win32",
      userAgentData: {
        platform: "Windows",
        mobile: false,
        getHighEntropyValues: vi.fn().mockReturnValue(highEntropyValues),
      },
    });
    render(<App />);

    const chooser = await screen.findByRole("link", { name: /Review Windows downloads/i });
    act(() => chooser.focus());

    await act(async () => {
      resolveHints({ architecture: "x86", bitness: "64", platformVersion: "13.0.0" });
      await highEntropyValues;
    });

    expect(chooser).toHaveFocus();
    expect(chooser).toHaveAttribute("href", "#install");
    expect(chooser).toHaveTextContent("Review Windows downloads");

    act(() => chooser.blur());
    await waitFor(() => {
      expect(screen.getByRole("link", { name: /Download for Windows/i })).toHaveAttribute(
        "href",
        `${ASSET_URL}/Aseprite-Installer-Windows-x64-setup.exe`,
      );
    });
  });

  it("does not treat keyboard focus on an automatic tab as a manual selection", async () => {
    mockNavigator({
      userAgent:
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140 Safari/537.36",
      platform: "Win32",
      userAgentData: { platform: "Windows", mobile: false },
    });
    render(<App />);

    const windowsTab = screen.getByRole("tab", { name: "Windows" });
    await waitFor(() => expect(windowsTab).toHaveAttribute("aria-selected", "true"));
    act(() => windowsTab.focus());

    expect(screen.getByRole("status")).not.toHaveTextContent("selected manually");
    expect(screen.getByRole("status")).toHaveTextContent(
      "Windows detected · confirm a Windows 11 x64 system",
    );
  });

  it("blocks a direct download for an incompatible Windows version", async () => {
    mockNavigator({
      userAgent:
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140 Safari/537.36",
      platform: "Win32",
      userAgentData: {
        platform: "Windows",
        mobile: false,
        getHighEntropyValues: vi.fn().mockResolvedValue({
          architecture: "x86",
          bitness: "64",
          platformVersion: "10.0.0",
        }),
      },
    });
    render(<App />);

    const chooser = await screen.findByRole("link", { name: /Review Windows downloads/i });
    expect(chooser).toHaveAttribute("href", "#install");
    expect(screen.getByRole("status")).toHaveTextContent(
      "Windows 10 detected · Windows 11 is required",
    );
  });

  it("freezes the CTA from pointer down until an in-flight detection is safe to apply", async () => {
    type WindowsHints = { architecture: string; bitness: string; platformVersion: string };
    let resolveHints: (value: WindowsHints) => void = () => undefined;
    const highEntropyValues = new Promise<WindowsHints>((resolve) => {
      resolveHints = resolve;
    });
    mockNavigator({
      userAgent:
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140 Safari/537.36",
      platform: "Win32",
      userAgentData: {
        platform: "Windows",
        mobile: false,
        getHighEntropyValues: vi.fn().mockReturnValue(highEntropyValues),
      },
    });
    render(<App />);

    const chooser = await screen.findByRole("link", { name: /Review Windows downloads/i });
    fireEvent.pointerDown(chooser);
    await act(async () => {
      resolveHints({ architecture: "x86", bitness: "64", platformVersion: "13.0.0" });
      await highEntropyValues;
    });

    expect(chooser).toHaveAttribute("href", "#install");
    expect(chooser).toHaveTextContent("Review Windows downloads");

    fireEvent.pointerCancel(chooser);
    await waitFor(() => {
      expect(screen.getByRole("link", { name: /Download for Windows/i })).toHaveAttribute(
        "href",
        `${ASSET_URL}/Aseprite-Installer-Windows-x64-setup.exe`,
      );
    });
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
