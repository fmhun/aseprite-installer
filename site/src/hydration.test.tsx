import { act } from "react";
import { StrictMode } from "react";
import { hydrateRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { render } from "./entry-server";

const matchMedia = vi.fn().mockReturnValue({
  matches: false,
  media: "(prefers-reduced-motion: reduce)",
  onchange: null,
  addEventListener: vi.fn(),
  removeEventListener: vi.fn(),
  addListener: vi.fn(),
  removeListener: vi.fn(),
  dispatchEvent: vi.fn(),
});
const navigatorKeys = ["userAgent", "platform", "maxTouchPoints", "userAgentData"] as const;
const originalNavigatorDescriptors = new Map(
  navigatorKeys.map((key) => [key, Object.getOwnPropertyDescriptor(window.navigator, key)]),
);

function restoreNavigator() {
  for (const key of navigatorKeys) {
    const descriptor = originalNavigatorDescriptors.get(key);
    if (descriptor) Object.defineProperty(window.navigator, key, descriptor);
    else Reflect.deleteProperty(window.navigator, key);
  }
}

describe("static landing hydration", () => {
  let root: Root | undefined;
  const reactTestGlobal = globalThis as typeof globalThis & {
    IS_REACT_ACT_ENVIRONMENT?: boolean;
  };

  beforeEach(() => {
    reactTestGlobal.IS_REACT_ACT_ENVIRONMENT = true;
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      writable: true,
      value: matchMedia,
    });
    Object.defineProperties(window.navigator, {
      userAgent: {
        configurable: true,
        value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/140 Safari/537.36",
      },
      platform: { configurable: true, value: "Win32" },
      maxTouchPoints: { configurable: true, value: 0 },
      userAgentData: {
        configurable: true,
        value: {
          platform: "Windows",
          mobile: false,
          getHighEntropyValues: vi.fn().mockResolvedValue({
            architecture: "x86",
            bitness: "64",
            platformVersion: "13.0.0",
          }),
        },
      },
    });
  });

  afterEach(async () => {
    if (root) {
      await act(async () => root?.unmount());
      root = undefined;
    }
    restoreNavigator();
    reactTestGlobal.IS_REACT_ACT_ENVIRONMENT = false;
  });

  it("hydrates the pre-rendered HTML without a server/client mismatch", async () => {
    document.body.innerHTML = `<div id="root" data-prerendered="true">${render()}</div>`;
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);

    await act(async () => {
      root = hydrateRoot(
        document.getElementById("root")!,
        <StrictMode>
          <App />
        </StrictMode>,
      );
    });

    expect(consoleError).not.toHaveBeenCalled();
    expect(document.querySelector("h1")).toHaveTextContent(/Install Aseprite\s*from source\./);
    await vi.waitFor(() => {
      expect(document.querySelector('[data-download-target="windows-x64"]')).toHaveAttribute(
        "href",
        "https://github.com/fmhun/aseprite-installer/releases/latest/download/Aseprite-Installer-Windows-x64-setup.exe",
      );
    });
  });
});
