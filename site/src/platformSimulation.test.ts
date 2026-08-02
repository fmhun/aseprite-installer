import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getCompatibleDownloadTarget } from "./platformDetection";
import {
  getPlatformSimulationSnapshot,
  getPlatformSimulationState,
  getSimulatedPlatformDetection,
  installPlatformSimulationConsole,
  listPlatformSimulations,
  platformSimulationIds,
  resetPlatformSimulation,
  simulatePlatform,
  subscribePlatformSimulation,
  type AsepriteInstallerConsoleApi,
} from "./platformSimulation";

const storageKey = "aseprite-installer:platform-simulation:v1";
let uninstallConsole: (() => void) | undefined;

describe("production platform simulation", () => {
  beforeEach(() => {
    Reflect.deleteProperty(window, "AsepriteInstaller");
    sessionStorage.clear();
    resetPlatformSimulation();
  });

  afterEach(() => {
    uninstallConsole?.();
    uninstallConsole = undefined;
    Reflect.deleteProperty(window, "AsepriteInstaller");
    resetPlatformSimulation();
    sessionStorage.clear();
    vi.restoreAllMocks();
  });

  it.each([
    ["macos-arm64", "macos-arm64"],
    ["macos-x64", "macos-x64"],
    ["windows-x64", "windows-x64"],
    ["linux-x64", "linux-x64"],
  ] as const)("maps %s to its exact direct download", (id, expectedTarget) => {
    const state = simulatePlatform(id);

    expect(state.active).toBe(true);
    expect(state.id).toBe(id);
    expect(state.detection).not.toBeNull();
    expect(getCompatibleDownloadTarget(state.detection!)).toBe(expectedTarget);
    expect(getSimulatedPlatformDetection()).toBe(state.detection);
  });

  it.each([
    "macos-unknown",
    "macos-15.1",
    "windows-arm64",
    "windows-10",
    "linux-arm64",
    "android",
    "ios",
    "ipados",
    "chromeos",
    "mobile",
    "unknown",
  ] as const)("keeps %s away from an incompatible direct package", (id) => {
    const state = simulatePlatform(id);

    expect(state.detection).not.toBeNull();
    expect(getCompatibleDownloadTarget(state.detection!)).toBeNull();
  });

  it.each([
    ["macos", "macos-arm64"],
    ["windows", "windows-x64"],
    ["linux", "linux-x64"],
  ] as const)("provides the ergonomic %s alias", (alias, expectedId) => {
    expect(simulatePlatform(alias).id).toBe(expectedId);
  });

  it("publishes stable snapshots synchronously, including repeated commands", () => {
    const listener = vi.fn();
    const unsubscribe = subscribePlatformSimulation(listener);
    const before = getPlatformSimulationSnapshot();

    const first = simulatePlatform("linux");
    const firstSnapshot = getPlatformSimulationSnapshot();
    const second = simulatePlatform("linux");
    const secondSnapshot = getPlatformSimulationSnapshot();

    expect(first.revision).toBe(before.revision + 1);
    expect(second.revision).toBe(first.revision + 1);
    expect(firstSnapshot).not.toBe(secondSnapshot);
    expect(getPlatformSimulationSnapshot()).toBe(secondSnapshot);
    expect(listener).toHaveBeenCalledTimes(2);

    unsubscribe();
    resetPlatformSimulation();
    expect(listener).toHaveBeenCalledTimes(2);
  });

  it("persists only when session persistence is explicitly requested", () => {
    const memoryState = simulatePlatform("linux");
    expect(memoryState.persistence).toBe("memory");
    expect(memoryState.persisted).toBe(false);
    expect(sessionStorage.getItem(storageKey)).toBeNull();

    const sessionState = simulatePlatform("windows", { persist: "session" });
    expect(sessionState.persistence).toBe("session");
    expect(sessionState.persisted).toBe(true);
    expect(JSON.parse(sessionStorage.getItem(storageKey)!)).toEqual({
      id: "windows-x64",
      version: 1,
    });

    simulatePlatform("macos");
    expect(sessionStorage.getItem(storageKey)).toBeNull();
    resetPlatformSimulation();
    expect(getPlatformSimulationState()).toMatchObject({
      active: false,
      id: null,
      persisted: false,
    });
  });

  it("restores a valid session simulation in a freshly loaded module", async () => {
    sessionStorage.setItem(
      storageKey,
      JSON.stringify({ id: "macos-x64", version: 1 }),
    );
    vi.resetModules();

    const freshModule = await import("./platformSimulation");

    expect(freshModule.getPlatformSimulationSnapshot()).toMatchObject({
      id: "macos-x64",
      persistence: "session",
      revision: 0,
    });
    expect(freshModule.getPlatformSimulationState()).toMatchObject({
      active: true,
      id: "macos-x64",
      persisted: true,
    });
    freshModule.resetPlatformSimulation();
  });

  it.each([
    "not-json",
    JSON.stringify({ id: "plan9", version: 1 }),
    JSON.stringify({ id: "linux-x64", version: 2 }),
  ])("ignores and removes invalid persisted state: %s", async (storedValue) => {
    sessionStorage.setItem(storageKey, storedValue);
    vi.resetModules();

    const freshModule = await import("./platformSimulation");

    expect(freshModule.getPlatformSimulationSnapshot().id).toBeNull();
    expect(sessionStorage.getItem(storageKey)).toBeNull();
  });

  it("falls back to memory if sessionStorage is unavailable", () => {
    const descriptor = Object.getOwnPropertyDescriptor(window, "sessionStorage");
    Object.defineProperty(window, "sessionStorage", {
      configurable: true,
      get: () => {
        throw new DOMException("Blocked", "SecurityError");
      },
    });

    try {
      expect(simulatePlatform("linux", { persist: "session" })).toMatchObject({
        active: true,
        id: "linux-x64",
        persisted: false,
        persistence: "memory",
      });
    } finally {
      if (descriptor) Object.defineProperty(window, "sessionStorage", descriptor);
    }
  });

  it("keeps a session override unchanged when storage cannot be cleared", () => {
    const initial = simulatePlatform("windows", { persist: "session" });
    const initialSnapshot = getPlatformSimulationSnapshot();
    const descriptor = Object.getOwnPropertyDescriptor(window, "sessionStorage");
    Object.defineProperty(window, "sessionStorage", {
      configurable: true,
      get: () => {
        throw new DOMException("Blocked", "SecurityError");
      },
    });

    try {
      expect(() => resetPlatformSimulation()).toThrow(
        "The persisted platform simulation could not be cleared",
      );
      expect(getPlatformSimulationSnapshot()).toBe(initialSnapshot);
      expect(getPlatformSimulationState()).toEqual(initial);
    } finally {
      if (descriptor) Object.defineProperty(window, "sessionStorage", descriptor);
    }
  });

  it("keeps a session override unchanged when storage cannot be replaced", () => {
    const initial = simulatePlatform("windows", { persist: "session" });
    const initialSnapshot = getPlatformSimulationSnapshot();
    const descriptor = Object.getOwnPropertyDescriptor(window, "sessionStorage");
    Object.defineProperty(window, "sessionStorage", {
      configurable: true,
      get: () => {
        throw new DOMException("Blocked", "SecurityError");
      },
    });

    try {
      expect(() => simulatePlatform("linux", { persist: "session" })).toThrow(
        "The persisted platform simulation could not be replaced",
      );
      expect(getPlatformSimulationSnapshot()).toBe(initialSnapshot);
      expect(getPlatformSimulationState()).toEqual(initial);
    } finally {
      if (descriptor) Object.defineProperty(window, "sessionStorage", descriptor);
    }
  });

  it("rejects invalid input atomically", () => {
    const initial = simulatePlatform("windows");
    const initialSnapshot = getPlatformSimulationSnapshot();

    expect(() => simulatePlatform("plan9" as never)).toThrow(TypeError);
    expect(() => simulatePlatform("linux", { persist: "forever" } as never)).toThrow(
      TypeError,
    );
    expect(getPlatformSimulationSnapshot()).toBe(initialSnapshot);
    expect(getPlatformSimulationState()).toEqual(initial);
    expect(sessionStorage.getItem(storageKey)).toBeNull();
  });

  it("returns immutable state, detections, and scenario metadata", () => {
    const state = simulatePlatform("macos-x64");
    const list = listPlatformSimulations();

    expect(Object.isFrozen(state)).toBe(true);
    expect(Object.isFrozen(state.detection)).toBe(true);
    expect(Object.isFrozen(list)).toBe(true);
    expect(list).toHaveLength(platformSimulationIds.length);
    expect(list.every(Object.isFrozen)).toBe(true);
  });

  it("installs an idempotent, non-writable console facade with help", () => {
    const consoleInfo = vi.spyOn(console, "info").mockImplementation(() => undefined);
    const consoleTable = vi.spyOn(console, "table").mockImplementation(() => undefined);
    uninstallConsole = installPlatformSimulationConsole();
    const namespace = window.AsepriteInstaller;
    const secondCleanup = installPlatformSimulationConsole();

    expect(namespace).toBeDefined();
    expect(window.AsepriteInstaller).toBe(namespace);
    expect(Object.isFrozen(namespace)).toBe(true);
    expect(Object.isFrozen(namespace?.platform)).toBe(true);
    expect(Object.getOwnPropertyDescriptor(window, "AsepriteInstaller")).toMatchObject({
      configurable: true,
      enumerable: false,
      writable: false,
    });
    expect(namespace?.platform.simulate("linux").id).toBe("linux-x64");
    expect(namespace?.platform.help()).toBe(listPlatformSimulations());
    expect(consoleInfo).toHaveBeenCalledOnce();
    expect(consoleTable).toHaveBeenCalledWith(listPlatformSimulations());

    secondCleanup();
    expect(window.AsepriteInstaller).toBe(namespace);
  });

  it("does not overwrite a foreign console namespace", () => {
    const consoleWarn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const foreign = Object.freeze({ platform: Object.freeze({}) }) as unknown as
      Readonly<AsepriteInstallerConsoleApi>;
    Object.defineProperty(window, "AsepriteInstaller", {
      configurable: true,
      value: foreign,
    });

    const cleanup = installPlatformSimulationConsole();
    expect(window.AsepriteInstaller).toBe(foreign);
    expect(consoleWarn).toHaveBeenCalledWith(
      "Aseprite Installer platform simulation was not installed because window.AsepriteInstaller already exists.",
    );
    cleanup();
    expect(window.AsepriteInstaller).toBe(foreign);
  });
});
