import type { PlatformDetection } from "./platformDetection";

export const platformSimulationIds = Object.freeze([
  "macos-arm64",
  "macos-x64",
  "macos-unknown",
  "macos-15.1",
  "windows-x64",
  "windows-arm64",
  "windows-10",
  "linux-x64",
  "linux-arm64",
  "android",
  "ios",
  "ipados",
  "chromeos",
  "mobile",
  "unknown",
] as const);

export type PlatformSimulationId = (typeof platformSimulationIds)[number];
export type PlatformSimulationInput =
  | PlatformSimulationId
  | "macos"
  | "windows"
  | "linux";
export type PlatformSimulationPersistence = "memory" | "session";

export interface PlatformSimulationOptions {
  persist?: PlatformSimulationPersistence;
}

export interface PlatformSimulationSnapshot {
  readonly id: PlatformSimulationId | null;
  readonly persistence: PlatformSimulationPersistence | null;
  readonly revision: number;
}

export interface PlatformSimulationListItem {
  readonly id: PlatformSimulationId;
  readonly label: string;
  readonly result: string;
}

export interface PlatformSimulationState {
  readonly active: boolean;
  readonly detection: Readonly<PlatformDetection> | null;
  readonly id: PlatformSimulationId | null;
  readonly label: string | null;
  readonly persisted: boolean;
  readonly persistence: PlatformSimulationPersistence | null;
  readonly revision: number;
}

export interface PlatformSimulationConsoleApi {
  readonly simulate: (
    input: PlatformSimulationInput,
    options?: PlatformSimulationOptions,
  ) => PlatformSimulationState;
  readonly reset: () => PlatformSimulationState;
  readonly state: () => PlatformSimulationState;
  readonly list: () => readonly PlatformSimulationListItem[];
  readonly help: () => readonly PlatformSimulationListItem[];
}

export interface AsepriteInstallerConsoleApi {
  readonly platform: Readonly<PlatformSimulationConsoleApi>;
}

declare global {
  interface Window {
    readonly AsepriteInstaller?: Readonly<AsepriteInstallerConsoleApi>;
  }
}

interface PlatformSimulationScenario {
  readonly detection: Readonly<PlatformDetection>;
  readonly label: string;
  readonly result: string;
}

const STORAGE_KEY = "aseprite-installer:platform-simulation:v1";
const STORAGE_VERSION = 1;
const CONSOLE_API_MARKER = Symbol.for("aseprite-installer.platform-simulation-console");
const validSimulationIds = new Set<string>(platformSimulationIds);
const listeners = new Set<() => void>();

const autoSnapshot: PlatformSimulationSnapshot = Object.freeze({
  id: null,
  persistence: null,
  revision: 0,
});

let clientSnapshot: PlatformSimulationSnapshot | undefined;

function freezeDetection(detection: PlatformDetection): Readonly<PlatformDetection> {
  return Object.freeze(detection);
}

function desktopDetection(
  platform: "macos" | "windows" | "linux",
  architecture: PlatformDetection["architecture"],
  platformVersion: string | null,
  osCompatibility: PlatformDetection["osCompatibility"],
): Readonly<PlatformDetection> {
  return freezeDetection({
    platform,
    architecture,
    platformConfidence: "high",
    architectureConfidence: architecture === "unknown" ? "low" : "high",
    platformVersion,
    osCompatibility,
    hasConflict: false,
    deviceKind: "desktop",
    exclusionReason: null,
    source: "simulation",
  });
}

function excludedDetection(
  deviceKind: "mobile" | "unsupported" | "unknown",
  exclusionReason: PlatformDetection["exclusionReason"],
): Readonly<PlatformDetection> {
  return freezeDetection({
    platform: null,
    architecture: "unknown",
    platformConfidence: "low",
    architectureConfidence: "low",
    platformVersion: null,
    osCompatibility: "unknown",
    hasConflict: false,
    deviceKind,
    exclusionReason,
    source: "simulation",
  });
}

function scenario(
  label: string,
  result: string,
  detection: Readonly<PlatformDetection>,
): PlatformSimulationScenario {
  return Object.freeze({ detection, label, result });
}

const scenarios: Readonly<Record<PlatformSimulationId, PlatformSimulationScenario>> =
  Object.freeze({
    "macos-arm64": scenario(
      "macOS 15.2+ · Apple Silicon",
      "Direct Apple Silicon DMG",
      desktopDetection("macos", "arm64", "15.2.0", "compatible"),
    ),
    "macos-x64": scenario(
      "macOS 15.2+ · Intel",
      "Direct Intel DMG",
      desktopDetection("macos", "x64", "15.2.0", "compatible"),
    ),
    "macos-unknown": scenario(
      "macOS · architecture hidden",
      "Architecture picker",
      desktopDetection("macos", "unknown", "15.2.0", "compatible"),
    ),
    "macos-15.1": scenario(
      "macOS 15.1 · unsupported",
      "Minimum-version warning",
      desktopDetection("macos", "arm64", "15.1.0", "incompatible"),
    ),
    "windows-x64": scenario(
      "Windows 11 · x64",
      "Direct NSIS installer",
      desktopDetection("windows", "x64", "13.0.0", "compatible"),
    ),
    "windows-arm64": scenario(
      "Windows 11 · ARM64",
      "Architecture warning",
      desktopDetection("windows", "arm64", "13.0.0", "compatible"),
    ),
    "windows-10": scenario(
      "Windows 10 · x64",
      "Minimum-version warning",
      desktopDetection("windows", "x64", "10.0.0", "incompatible"),
    ),
    "linux-x64": scenario(
      "Linux · x86_64",
      "Direct AppImage",
      desktopDetection("linux", "x64", null, "unknown"),
    ),
    "linux-arm64": scenario(
      "Linux · ARM64",
      "Architecture warning",
      desktopDetection("linux", "arm64", null, "unknown"),
    ),
    android: scenario(
      "Android",
      "Desktop platform picker",
      excludedDetection("mobile", "android"),
    ),
    ios: scenario(
      "iOS",
      "Desktop platform picker",
      excludedDetection("mobile", "ios"),
    ),
    ipados: scenario(
      "iPadOS desktop mode",
      "Desktop platform picker",
      excludedDetection("mobile", "ipados"),
    ),
    chromeos: scenario(
      "ChromeOS",
      "Unsupported-system picker",
      excludedDetection("unsupported", "chromeos"),
    ),
    mobile: scenario(
      "Other mobile client",
      "Desktop platform picker",
      excludedDetection("mobile", "other-mobile"),
    ),
    unknown: scenario(
      "Unknown client",
      "Neutral platform picker",
      excludedDetection("unknown", null),
    ),
  });

const aliases: Readonly<Record<"macos" | "windows" | "linux", PlatformSimulationId>> =
  Object.freeze({
    macos: "macos-arm64",
    windows: "windows-x64",
    linux: "linux-x64",
  });

const simulationList: readonly PlatformSimulationListItem[] = Object.freeze(
  platformSimulationIds.map((id) => Object.freeze({
    id,
    label: scenarios[id].label,
    result: scenarios[id].result,
  })),
);

function getSessionStorage(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

function removePersistedSimulation(storage = getSessionStorage()): boolean {
  if (!storage) return false;
  try {
    storage.removeItem(STORAGE_KEY);
    return true;
  } catch {
    // Storage can be unavailable in hardened or private browsing contexts.
    return false;
  }
}

function readPersistedSimulation(): PlatformSimulationId | null {
  const storage = getSessionStorage();
  if (!storage) return null;

  try {
    const rawValue = storage.getItem(STORAGE_KEY);
    if (!rawValue) return null;
    const parsed = JSON.parse(rawValue) as { id?: unknown; version?: unknown };
    if (
      parsed.version === STORAGE_VERSION &&
      typeof parsed.id === "string" &&
      validSimulationIds.has(parsed.id)
    ) {
      return parsed.id as PlatformSimulationId;
    }
  } catch {
    // Invalid or inaccessible storage is treated as no override.
  }

  removePersistedSimulation(storage);
  return null;
}

function persistSimulation(id: PlatformSimulationId): boolean {
  const storage = getSessionStorage();
  if (!storage) return false;
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify({ id, version: STORAGE_VERSION }));
    return true;
  } catch {
    return false;
  }
}

function initializeClientSnapshot(): PlatformSimulationSnapshot {
  if (clientSnapshot) return clientSnapshot;
  const persistedId = readPersistedSimulation();
  clientSnapshot = persistedId
    ? Object.freeze({ id: persistedId, persistence: "session", revision: 0 })
    : autoSnapshot;
  return clientSnapshot;
}

export function getPlatformSimulationSnapshot(): PlatformSimulationSnapshot {
  return typeof window === "undefined" ? autoSnapshot : initializeClientSnapshot();
}

export function getPlatformSimulationServerSnapshot(): PlatformSimulationSnapshot {
  return autoSnapshot;
}

export function subscribePlatformSimulation(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function normalizeSimulationId(input: PlatformSimulationInput): PlatformSimulationId {
  if (typeof input !== "string") {
    throw new TypeError("Platform simulation must be one of the listed string presets.");
  }
  if (Object.prototype.hasOwnProperty.call(aliases, input)) {
    return aliases[input as keyof typeof aliases];
  }
  if (validSimulationIds.has(input)) return input as PlatformSimulationId;
  throw new TypeError(
    `Unknown platform simulation "${input}". Run AsepriteInstaller.platform.help() for valid presets.`,
  );
}

function stateFromSnapshot(snapshot: PlatformSimulationSnapshot): PlatformSimulationState {
  const selectedScenario = snapshot.id ? scenarios[snapshot.id] : null;
  return Object.freeze({
    active: selectedScenario !== null,
    detection: selectedScenario?.detection ?? null,
    id: snapshot.id,
    label: selectedScenario?.label ?? null,
    persisted: snapshot.persistence === "session",
    persistence: snapshot.persistence,
    revision: snapshot.revision,
  });
}

function publishSimulation(
  id: PlatformSimulationId | null,
  requestedPersistence: PlatformSimulationPersistence | null,
): PlatformSimulationState {
  const current = getPlatformSimulationSnapshot();
  let persistence = requestedPersistence;

  if (id === null || requestedPersistence !== "session") {
    const storageCleared = removePersistedSimulation();
    if (!storageCleared && current.persistence === "session") {
      throw new Error(
        "The persisted platform simulation could not be cleared. Restore sessionStorage access and try again.",
      );
    }
  } else {
    const stored = persistSimulation(id);
    if (!stored && current.persistence === "session") {
      throw new Error(
        "The persisted platform simulation could not be replaced. Restore sessionStorage access and try again.",
      );
    }
    if (!stored) persistence = "memory";
  }

  clientSnapshot = Object.freeze({
    id,
    persistence: id === null ? null : persistence ?? "memory",
    revision: current.revision + 1,
  });
  for (const listener of [...listeners]) listener();
  return stateFromSnapshot(clientSnapshot);
}

export function simulatePlatform(
  input: PlatformSimulationInput,
  options: PlatformSimulationOptions = {},
): PlatformSimulationState {
  if (
    options === null ||
    typeof options !== "object" ||
    (options.persist !== undefined &&
      options.persist !== "memory" &&
      options.persist !== "session")
  ) {
    throw new TypeError('persist must be either "memory" or "session".');
  }
  const id = normalizeSimulationId(input);
  return publishSimulation(id, options.persist ?? "memory");
}

export function resetPlatformSimulation(): PlatformSimulationState {
  return publishSimulation(null, null);
}

export function getPlatformSimulationState(): PlatformSimulationState {
  return stateFromSnapshot(getPlatformSimulationSnapshot());
}

export function listPlatformSimulations(): readonly PlatformSimulationListItem[] {
  return simulationList;
}

export function getSimulatedPlatformDetection(
  snapshot = getPlatformSimulationSnapshot(),
): Readonly<PlatformDetection> | null {
  return snapshot.id ? scenarios[snapshot.id].detection : null;
}

export function installPlatformSimulationConsole(): () => void {
  const target = window;
  const currentNamespace = target.AsepriteInstaller as
    | (Readonly<AsepriteInstallerConsoleApi> & { [CONSOLE_API_MARKER]?: true })
    | undefined;
  if (currentNamespace?.[CONSOLE_API_MARKER]) return () => undefined;
  if (Object.prototype.hasOwnProperty.call(target, "AsepriteInstaller")) {
    console.warn(
      "Aseprite Installer platform simulation was not installed because window.AsepriteInstaller already exists.",
    );
    return () => undefined;
  }

  const platform: Readonly<PlatformSimulationConsoleApi> = Object.freeze({
    simulate: simulatePlatform,
    reset: resetPlatformSimulation,
    state: getPlatformSimulationState,
    list: listPlatformSimulations,
    help: () => {
      console.info(
        "Aseprite Installer platform simulation\n" +
        'AsepriteInstaller.platform.simulate("linux")\n' +
        'AsepriteInstaller.platform.simulate("macos-x64")\n' +
        'AsepriteInstaller.platform.simulate("windows", { persist: "session" })\n' +
        "AsepriteInstaller.platform.reset()",
      );
      console.table(simulationList);
      return simulationList;
    },
  });
  const namespace = Object.freeze({
    [CONSOLE_API_MARKER]: true as const,
    platform,
  });

  Object.defineProperty(target, "AsepriteInstaller", {
    configurable: true,
    enumerable: false,
    value: namespace,
    writable: false,
  });

  return () => {
    if (target.AsepriteInstaller === namespace) {
      Reflect.deleteProperty(target, "AsepriteInstaller");
    }
  };
}
