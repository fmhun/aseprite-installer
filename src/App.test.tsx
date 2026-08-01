import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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

vi.mock("./timing", () => ({
  withMinimumDuration: (operation: Promise<unknown>) => operation,
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

const missingPreflight = {
  ...preflight,
  ready: false,
  prerequisites: [
    {
      id: "cmake",
      label: "CMake",
      ok: false,
      required: true,
      detail: "Not found",
      remediation: "Install CMake from cmake.org or Homebrew.",
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

const managedInstallation = {
  ...manualInstallation,
  id: "managed",
  path: "/Users/test/Applications/Aseprite.app",
  version: "v1.3.18.1",
  versionExact: true,
  channel: "managed" as const,
  hasBackup: true,
  installedAt: "2026-07-30T12:00:00Z",
};

async function reachInstallConfirmation() {
  fireEvent.click(
    await screen.findByRole("button", { name: /^Compile a personal copy/ }),
  );
  fireEvent.click(
    await screen.findByRole("button", { name: /^Check requirements/ }),
  );
  fireEvent.click(
    await screen.findByRole("button", { name: /^Compile and install/ }),
  );
  fireEvent.click(
    await screen.findByRole("checkbox", {
      name: /I have read the Aseprite EULA/,
    }),
  );
  return screen.getByRole("button", { name: /^Accept and start/ });
}

function stepStates(stepper: HTMLElement): Array<string | null> {
  return Array.from(stepper.querySelectorAll("[data-state]"), (step) =>
    step.getAttribute("data-state"),
  );
}

describe("Aseprite Installer contextual flow", () => {
  afterEach(cleanup);

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
        name: "Aseprite is not installed",
      }),
    ).toBeInTheDocument();
    expect(api.listReleases).not.toHaveBeenCalled();
    expect(api.runPreflight).not.toHaveBeenCalled();
    expect(screen.queryByText(/Other detected installations/)).not.toBeInTheDocument();
    expect(
      screen.queryByText(/SHA-256/),
    ).not.toBeInTheDocument();
    const footer = screen.getByRole("contentinfo", { name: "Project links" });
    expect(footer).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Buy official Aseprite — $19.99+ ↗" })).toBeInTheDocument();
    expect(screen.getByText("Recommended")).toBeInTheDocument();
    expect(within(footer).getByText(/not a free edition of Aseprite/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Buy Aseprite" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Aseprite on GitHub" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Aseprite Installer on GitHub" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Buy official Aseprite — $19.99+ ↗" }));
    fireEvent.click(screen.getByRole("button", { name: "Buy Aseprite" }));
    fireEvent.click(screen.getByRole("button", { name: "Aseprite on GitHub" }));
    fireEvent.click(screen.getByRole("button", { name: "Aseprite Installer on GitHub" }));
    expect(api.openExternal).toHaveBeenNthCalledWith(1, "https://www.aseprite.org/buy/");
    expect(api.openExternal).toHaveBeenNthCalledWith(2, "https://www.aseprite.org/buy/");
    expect(api.openExternal).toHaveBeenNthCalledWith(3, "https://github.com/aseprite/aseprite");
    expect(api.openExternal).toHaveBeenNthCalledWith(4, "https://github.com/fmhun/asprite-installer");
  });

  it("shows a minimal installed state before entering the reinstall flow", async () => {
    vi.mocked(api.scanInstallations).mockResolvedValue([manualInstallation]);
    render(<App />);

    expect(
      await screen.findByRole("heading", {
        name: "Aseprite is already installed",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Aseprite 1.3 · arm64")).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: "Manage this installation",
      }),
    ).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Support Aseprite" })).toBeInTheDocument();
    expect(screen.getByText(/If this is a personal source build/)).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "Support Aseprite development ↗" }),
    );
    expect(api.openExternal).toHaveBeenCalledWith("https://www.aseprite.org/buy/");
    expect(api.listReleases).not.toHaveBeenCalled();
    expect(api.runPreflight).not.toHaveBeenCalled();
  });

  it("loads releases and requirements only when their steps are opened", async () => {
    render(<App />);
    const installButton = await screen.findByRole("button", {
      name: /^Compile a personal copy/,
    });
    fireEvent.click(installButton);

    const stepper = screen.getByRole("navigation", {
      name: "Installation steps",
    });
    expect(stepStates(stepper)).toEqual(["current", "upcoming", "upcoming"]);

    await waitFor(() => expect(api.listReleases).toHaveBeenCalledWith(false));
    expect(await screen.findByText(/SHA-256/)).toBeInTheDocument();
    expect(api.runPreflight).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getByRole("button", { name: /^Check requirements/ }),
    );
    await waitFor(() => expect(api.runPreflight).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("CMake")).toBeInTheDocument();
    expect(stepStates(stepper)).toEqual(["done", "current", "upcoming"]);

    fireEvent.click(screen.getByRole("button", { name: /Back/ }));
    expect(stepStates(stepper)).toEqual(["current", "upcoming", "upcoming"]);

    const checkAgain = await screen.findByRole("button", {
      name: /^Check requirements/,
    });
    await waitFor(() => expect(checkAgain).toBeEnabled());
    fireEvent.click(checkAgain);
    await waitFor(() => expect(api.runPreflight).toHaveBeenCalledTimes(2));
    expect(stepStates(stepper)).toEqual(["done", "current", "upcoming"]);
    expect(
      screen.getByRole("button", {
        name: /^Compile and install/,
      }),
    ).toBeEnabled();

    vi.mocked(api.startInstall).mockRejectedValue(new Error("Build stopped"));
    fireEvent.click(
      screen.getByRole("button", {
        name: /^Compile and install/,
      }),
    );
    fireEvent.click(
      await screen.findByRole("checkbox", {
        name: /I have read the Aseprite EULA/,
      }),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: /^Accept and start/,
      }),
    );
    expect(stepStates(stepper)).toEqual(["done", "done", "current"]);

    fireEvent.click(await screen.findByRole("button", { name: /Back/ }));
    expect(stepStates(stepper)).toEqual(["done", "current", "upcoming"]);
  });

  it("shows other installations only when more than one copy is detected", async () => {
    vi.mocked(api.scanInstallations).mockResolvedValue([
      managedInstallation,
      manualInstallation,
    ]);
    render(<App />);

    expect(
      await screen.findByText("Other detected installations (1)"),
    ).toBeInTheDocument();
  });

  it("shows current manual setup guidance for every missing requirement", async () => {
    vi.mocked(api.runPreflight).mockResolvedValue(missingPreflight);
    render(<App />);

    fireEvent.click(
      await screen.findByRole("button", { name: /^Compile a personal copy/ }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /^Check requirements/ }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Install manually" }),
    );

    const dialog = screen.getByRole("dialog", { name: "Install CMake" });
    expect(within(dialog).getByText("brew install cmake")).toBeInTheDocument();
    expect(
      within(dialog).getByRole("button", { name: /Official CMake downloads/ }),
    ).toBeInTheDocument();
  });

  it("rechecks requirements before opening the EULA and refreshes invalid state", async () => {
    vi.mocked(api.runPreflight)
      .mockResolvedValueOnce(preflight)
      .mockResolvedValueOnce(missingPreflight);
    render(<App />);

    fireEvent.click(
      await screen.findByRole("button", { name: /^Compile a personal copy/ }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /^Check requirements/ }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /^Compile and install/ }),
    );

    await waitFor(() => expect(api.runPreflight).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("dialog", { name: "Personal-use compilation" })).not.toBeInTheDocument();
    expect(await screen.findByText("Not found")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Check again/ })).toBeEnabled();
  });

  it("offers the official purchase path in the personal-build confirmation", async () => {
    render(<App />);

    fireEvent.click(
      await screen.findByRole("button", { name: /^Compile a personal copy/ }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /^Check requirements/ }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /^Compile and install/ }),
    );

    const dialog = await screen.findByRole("dialog", {
      name: "Personal-use compilation",
    });
    expect(
      within(dialog).getByText("Support the people who make Aseprite"),
    ).toBeInTheDocument();
    fireEvent.click(
      within(dialog).getByRole("button", {
        name: "Buy the official version instead ↗",
      }),
    );
    expect(api.openExternal).toHaveBeenCalledWith("https://www.aseprite.org/buy/");
  });

  it("stops a failed installation and offers a working retry", async () => {
    vi.mocked(api.startInstall)
      .mockRejectedValueOnce("Quit Aseprite before replacing, restoring, or removing it.")
      .mockResolvedValueOnce(managedInstallation);
    render(<App />);

    fireEvent.click(await reachInstallConfirmation());

    expect(
      await screen.findByRole("heading", { name: "Installation stopped" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Quit Aseprite before replacing, restoring, or removing it."),
    ).toBeInTheDocument();
    const progressFill = screen.getByRole("progressbar").firstElementChild;
    expect(progressFill).not.toHaveClass("indeterminate");
    expect(screen.getByRole("button", { name: "Try again" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(
      await screen.findByRole("heading", { name: "Aseprite is ready" }),
    ).toBeInTheDocument();
    expect(
      stepStates(
        screen.getByRole("navigation", { name: "Installation steps" }),
      ),
    ).toEqual(["done", "done", "complete"]);
    expect(screen.getByText(/consider buying an official copy/)).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "Support Aseprite development ↗" }),
    );
    expect(api.openExternal).toHaveBeenCalledWith("https://www.aseprite.org/buy/");
    expect(api.startInstall).toHaveBeenCalledTimes(2);
  });

  it("keeps the technical log visible and non-collapsible while installing", async () => {
    vi.mocked(api.startInstall).mockImplementation(() => new Promise(() => {}));
    render(<App />);

    fireEvent.click(await reachInstallConfirmation());

    const logHeading = await screen.findByRole("heading", {
      name: "Logs",
    });
    expect(logHeading.closest("section")).toHaveClass("logs");
    expect(document.querySelector("details.logs")).not.toBeInTheDocument();
  });

  it("restores a backup through an explicit in-app confirmation", async () => {
    vi.mocked(api.scanInstallations).mockResolvedValue([managedInstallation]);
    vi.mocked(api.restorePrevious).mockResolvedValue(managedInstallation);
    render(<App />);

    fireEvent.click(await screen.findByText("More options"));
    fireEvent.click(screen.getByRole("button", { name: "Restore previous" }));
    const dialog = screen.getByRole("dialog", {
      name: "Restore the previous installation?",
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Restore previous" }),
    );

    await waitFor(() =>
      expect(api.restorePrevious).toHaveBeenCalledWith(managedInstallation.id),
    );
    expect(
      await screen.findByText("The previous installation was restored."),
    ).toBeInTheDocument();
  });

  it("uninstalls a managed app through an explicit confirmation", async () => {
    vi.mocked(api.scanInstallations).mockResolvedValue([managedInstallation]);
    vi.mocked(api.uninstallManaged).mockResolvedValue();
    render(<App />);

    fireEvent.click(await screen.findByText("More options"));
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    const dialog = screen.getByRole("dialog", {
      name: "Uninstall Aseprite?",
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Uninstall" }),
    );

    await waitFor(() =>
      expect(api.uninstallManaged).toHaveBeenCalledWith(managedInstallation.id),
    );
    expect(
      await screen.findByText("The managed application was moved to the Trash."),
    ).toBeInTheDocument();
  });

  it("keeps action errors visible inside the confirmation dialog", async () => {
    vi.mocked(api.scanInstallations).mockResolvedValue([managedInstallation]);
    vi.mocked(api.restorePrevious).mockRejectedValue(
      "Quit Aseprite before replacing, restoring, or removing it.",
    );
    render(<App />);

    fireEvent.click(await screen.findByText("More options"));
    fireEvent.click(screen.getByRole("button", { name: "Restore previous" }));
    const dialog = screen.getByRole("dialog", {
      name: "Restore the previous installation?",
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Restore previous" }),
    );

    expect(
      await within(dialog).findByRole("alert"),
    ).toHaveTextContent("Quit Aseprite before replacing, restoring, or removing it.");
    expect(
      within(dialog).getByRole("button", { name: "Restore previous" }),
    ).toBeEnabled();
  });
});
