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
  });

  afterEach(async () => {
    if (root) {
      await act(async () => root?.unmount());
      root = undefined;
    }
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
  });
});
