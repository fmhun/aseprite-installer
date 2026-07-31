import { render, screen, waitFor } from "@testing-library/react";
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

describe("Aseprite Installer UI", () => {
  beforeEach(() => {
    vi.mocked(api.listReleases).mockResolvedValue([release]);
    vi.mocked(api.scanInstallations).mockResolvedValue([]);
    vi.mocked(api.runPreflight).mockResolvedValue(preflight);
  });

  it("shows a verified release and fresh-install action", async () => {
    render(<App />);
    await waitFor(() =>
      expect(screen.getByText("Aseprite v1.3.18.1")).toBeInTheDocument(),
    );
    expect(screen.getByText(/SHA-256/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Compile and install|Compiler et installer/ }),
    ).toBeEnabled();
  });

  it("shows manual installations as adoptable", async () => {
    vi.mocked(api.scanInstallations).mockResolvedValue([
      {
        id: "manual",
        path: "/Applications/Aseprite.app",
        version: "1.3",
        versionExact: false,
        architecture: "arm64",
        channel: "manual",
        manageable: true,
        writable: true,
        hasBackup: false,
        installedAt: null,
      },
    ]);
    render(<App />);
    expect(
      await screen.findByRole("button", { name: /Adopt and manage|Adopter et gérer/ }),
    ).toBeInTheDocument();
  });
});
