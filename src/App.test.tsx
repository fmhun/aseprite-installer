import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { api } from "./api";

vi.mock("./api", () => ({
  api: {
    listReleases: vi.fn(),
    scanInstallations: vi.fn(),
    runPreflight: vi.fn(),
    installBuildTools: vi.fn(),
    startInstall: vi.fn(),
    cancelOperation: vi.fn(),
    launchInstallation: vi.fn(),
    revealInstallation: vi.fn(),
    restorePrevious: vi.fn(),
    uninstallManaged: vi.fn(),
    cleanCache: vi.fn(),
    openExternal: vi.fn(),
  },
}));

const release = {
  tag: "v1.3.18.1",
  name: "Aseprite v1.3.18.1",
  publishedAt: "2026-07-23T21:23:49Z",
  prerelease: false,
  latest: true,
  sourceAssetName: "Aseprite-v1.3.18.1-Source.zip",
  sourceUrl:
    "https://github.com/aseprite/aseprite/releases/download/v1.3.18.1/Aseprite-v1.3.18.1-Source.zip",
  digest: `sha256:${"a".repeat(64)}`,
  size: 80_144_453,
};

const preflight = {
  ready: true,
  architecture: "arm64",
  osVersion: "26.5.2",
  freeBytes: 20_000_000_000,
  minimumFreeBytes: 6_442_450_944,
  homebrewAvailable: true,
  prerequisites: [
    {
      id: "cmake",
      label: "CMake",
      ok: true,
      required: true,
      detail: "/opt/homebrew/bin/cmake",
      remediation: null,
    },
  ],
};

const manualInstallation = {
  id: "manual",
  path: "/Applications/Aseprite.app",
  version: "1.3",
  versionExact: false,
  architecture: "arm64",
  channel: "manual" as const,
  manageable: true,
  writable: true,
  hasBackup: false,
  installedAt: null,
};

describe("Aseprite Installer contextual flow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.listReleases).mockResolvedValue([release]);
    vi.mocked(api.scanInstallations).mockResolvedValue([]);
    vi.mocked(api.runPreflight).mockResolvedValue(preflight);
  });

  it("only scans installations on the initial empty-machine screen", async () => {
    render(<App />);

    expect(
      await screen.findByRole("heading", {
        name: /Aseprite is not installed|Aseprite n’est pas installé/,
      }),
    ).toBeInTheDocument();
    expect(api.listReleases).not.toHaveBeenCalled();
    expect(api.runPreflight).not.toHaveBeenCalled();
    expect(
      screen.queryByText(/SHA-256/),
    ).not.toBeInTheDocument();
  });

  it("shows a minimal installed state before entering the reinstall flow", async () => {
    vi.mocked(api.scanInstallations).mockResolvedValue([manualInstallation]);
    render(<App />);

    expect(
      await screen.findByRole("heading", {
        name: /already installed|déjà installé/,
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Aseprite 1.3 · arm64")).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: /Manage this installation|Gérer cette installation/,
      }),
    ).toBeInTheDocument();
    expect(api.listReleases).not.toHaveBeenCalled();
    expect(api.runPreflight).not.toHaveBeenCalled();
  });

  it("loads releases and requirements only when their steps are opened", async () => {
    render(<App />);
    const installButton = await screen.findByRole("button", {
      name: /Install Aseprite|Installer Aseprite/,
    });
    fireEvent.click(installButton);

    await waitFor(() => expect(api.listReleases).toHaveBeenCalledWith(false));
    expect(await screen.findByText(/SHA-256/)).toBeInTheDocument();
    expect(api.runPreflight).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getByRole("button", { name: /Check this Mac|Vérifier ce Mac/ }),
    );
    await waitFor(() => expect(api.runPreflight).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("CMake")).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: /Compile and install|Compiler et installer/,
      }),
    ).toBeEnabled();
  });
});
