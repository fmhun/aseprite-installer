export const supportedPlatforms = ["macos", "windows", "linux"] as const;

export type SupportedPlatform = (typeof supportedPlatforms)[number];
export type CpuArchitecture = "arm64" | "x64" | "x86" | "unknown";
export type DetectionConfidence = "high" | "medium" | "low";
export type OsCompatibility = "compatible" | "incompatible" | "unknown";
export type DeviceKind = "desktop" | "mobile" | "unsupported" | "unknown";
export type ExclusionReason =
  | "android"
  | "ios"
  | "ipados"
  | "chromeos"
  | "other-mobile"
  | "other-unsupported"
  | null;
export type DetectionSource =
  | "ua-client-hints"
  | "legacy-platform"
  | "user-agent"
  | "simulation"
  | "none";

export type DownloadTarget =
  | "macos-arm64"
  | "macos-x64"
  | "windows-x64"
  | "linux-x64";

interface UserAgentHighEntropyValues {
  architecture?: string;
  bitness?: string;
  platformVersion?: string;
  wow64?: boolean;
}

export interface UserAgentDataLike {
  platform?: string;
  mobile?: boolean;
  getHighEntropyValues?: (
    hints: string[],
  ) => Promise<UserAgentHighEntropyValues>;
}

export interface NavigatorLike {
  userAgent?: string;
  platform?: string;
  maxTouchPoints?: number;
  userAgentData?: UserAgentDataLike;
}

export interface PlatformDetection {
  platform: SupportedPlatform | null;
  architecture: CpuArchitecture;
  platformConfidence: DetectionConfidence;
  architectureConfidence: DetectionConfidence;
  platformVersion: string | null;
  osCompatibility: OsCompatibility;
  hasConflict: boolean;
  deviceKind: DeviceKind;
  exclusionReason: ExclusionReason;
  source: DetectionSource;
}

type PlatformCandidate = SupportedPlatform | Exclude<ExclusionReason, null> | null;

const unknownDetection: PlatformDetection = {
  platform: null,
  architecture: "unknown",
  platformConfidence: "low",
  architectureConfidence: "low",
  platformVersion: null,
  osCompatibility: "unknown",
  hasConflict: false,
  deviceKind: "unknown",
  exclusionReason: null,
  source: "none",
};

function candidateFromPlatformLabel(value: string | undefined): PlatformCandidate {
  const normalized = value?.trim().toLowerCase() ?? "";
  if (!normalized) return null;
  if (/android/.test(normalized)) return "android";
  if (/ipad/.test(normalized)) return "ipados";
  if (/iphone|ipod|ios/.test(normalized)) return "ios";
  if (/chrome\s*os|chromeos|cros/.test(normalized)) return "chromeos";
  if (/macos|mac\s*os|^mac/.test(normalized)) return "macos";
  if (/windows|^win/.test(normalized)) return "windows";
  if (/linux/.test(normalized)) return "linux";
  return null;
}

function candidateFromUserAgent(userAgent: string): PlatformCandidate {
  if (/android/i.test(userAgent)) return "android";
  if (/ipad/i.test(userAgent)) return "ipados";
  if (/iphone|ipod/i.test(userAgent)) return "ios";
  if (/cros/i.test(userAgent)) return "chromeos";
  if (/iemobile|windows phone|webos|blackberry|bb10|opera mini|silk|mobile/i.test(userAgent)) return "other-mobile";
  if (/tizen|kaios|playstation|xbox|nintendo|smart-tv|smarttv/i.test(userAgent)) {
    return "other-unsupported";
  }
  if (/windows nt/i.test(userAgent)) return "windows";
  if (/macintosh|mac os x/i.test(userAgent)) return "macos";
  if (/linux/i.test(userAgent)) return "linux";
  return null;
}

function isSupportedPlatform(candidate: PlatformCandidate): candidate is SupportedPlatform {
  return candidate === "macos" || candidate === "windows" || candidate === "linux";
}

function classifyBasePlatform(navigatorLike: NavigatorLike): PlatformDetection {
  const userAgent = navigatorLike.userAgent ?? "";
  const legacyPlatform = navigatorLike.platform ?? "";
  const maxTouchPoints = navigatorLike.maxTouchPoints ?? 0;
  const uaDataCandidate = candidateFromPlatformLabel(navigatorLike.userAgentData?.platform);
  const legacyCandidate = candidateFromPlatformLabel(legacyPlatform);
  const userAgentCandidate = candidateFromUserAgent(userAgent);

  const hasMacSignal = [uaDataCandidate, legacyCandidate, userAgentCandidate].includes("macos");
  const looksLikeIPad = hasMacSignal && maxTouchPoints > 1;
  const mobileReason: ExclusionReason = looksLikeIPad
    ? "ipados"
    : uaDataCandidate === "android" || userAgentCandidate === "android"
      ? "android"
      : uaDataCandidate === "ipados" || userAgentCandidate === "ipados"
        ? "ipados"
        : uaDataCandidate === "ios" || userAgentCandidate === "ios"
          ? "ios"
          : navigatorLike.userAgentData?.mobile === true ||
              uaDataCandidate === "other-mobile" ||
              userAgentCandidate === "other-mobile"
            ? "other-mobile"
            : null;
  if (mobileReason) {
    const source: DetectionSource =
      navigatorLike.userAgentData?.mobile === true ||
      uaDataCandidate === "android" ||
      uaDataCandidate === "ios" ||
      uaDataCandidate === "ipados" ||
      uaDataCandidate === "other-mobile" ||
      (looksLikeIPad && uaDataCandidate === "macos")
        ? "ua-client-hints"
        : looksLikeIPad && legacyCandidate === "macos"
          ? "legacy-platform"
          : "user-agent";
    return {
      ...unknownDetection,
      deviceKind: "mobile",
      exclusionReason: mobileReason,
      source,
    };
  }

  if (
    uaDataCandidate === "chromeos" ||
    legacyCandidate === "chromeos" ||
    userAgentCandidate === "chromeos" ||
    uaDataCandidate === "other-unsupported" ||
    legacyCandidate === "other-unsupported" ||
    userAgentCandidate === "other-unsupported"
  ) {
    const isChromeOs = [uaDataCandidate, legacyCandidate, userAgentCandidate].includes("chromeos");
    const exclusion = isChromeOs ? "chromeos" : "other-unsupported";
    return {
      ...unknownDetection,
      deviceKind: "unsupported",
      exclusionReason: exclusion,
      source: uaDataCandidate === exclusion
        ? "ua-client-hints"
        : legacyCandidate === exclusion
          ? "legacy-platform"
          : "user-agent",
    };
  }

  if (isSupportedPlatform(uaDataCandidate)) {
    const fallbackCandidates = [legacyCandidate, userAgentCandidate].filter(isSupportedPlatform);
    const hasConflict = fallbackCandidates.some((candidate) => candidate !== uaDataCandidate);
    return {
      ...unknownDetection,
      platform: uaDataCandidate,
      platformConfidence: hasConflict ? "low" : "high",
      hasConflict,
      deviceKind: "desktop",
      source: "ua-client-hints",
    };
  }

  const legacyIsSupported = isSupportedPlatform(legacyCandidate);
  const userAgentIsSupported = isSupportedPlatform(userAgentCandidate);

  if (legacyIsSupported && userAgentIsSupported) {
    if (legacyCandidate !== userAgentCandidate) {
      return { ...unknownDetection, hasConflict: true };
    }
    return {
      ...unknownDetection,
      platform: legacyCandidate,
      platformConfidence: "medium",
      deviceKind: "desktop",
      source: "legacy-platform",
    };
  }

  if (legacyIsSupported || userAgentIsSupported) {
    const platform: SupportedPlatform | null = legacyIsSupported
      ? legacyCandidate
      : userAgentIsSupported
        ? userAgentCandidate
        : null;
    if (!platform) return unknownDetection;
    return {
      ...unknownDetection,
      platform,
      platformConfidence: "low",
      deviceKind: "desktop",
      source: legacyIsSupported ? "legacy-platform" : "user-agent",
    };
  }

  return unknownDetection;
}

function architectureFromClientHints(
  values: UserAgentHighEntropyValues,
): CpuArchitecture {
  const architecture = values.architecture?.trim().toLowerCase() ?? "";
  const bitness = values.bitness?.trim() ?? "";

  if (/arm64|aarch64/.test(architecture)) return "arm64";
  if (architecture === "arm" && bitness === "64") return "arm64";
  if (/x86_64|x86-64|amd64|x64/.test(architecture)) return "x64";
  if (architecture === "x86" && bitness === "64") return "x64";
  if (architecture === "x86" && bitness === "32" && values.wow64 === true) return "x64";
  if (architecture === "x86" && bitness === "32") return "x86";
  return "unknown";
}

function parseNumericVersion(value: string | undefined): number[] | null {
  const normalized = value?.trim() ?? "";
  if (!/^\d+(?:\.\d+)*$/.test(normalized)) return null;

  const parts = normalized.split(".").map(Number);
  return parts.every(Number.isSafeInteger) ? parts : null;
}

function compatibilityFromPlatformVersion(
  platform: SupportedPlatform,
  platformVersion: string | undefined,
): OsCompatibility {
  const parts = parseNumericVersion(platformVersion);
  if (!parts) return "unknown";

  if (platform === "windows") {
    const major = parts[0];
    if (major >= 13) return "compatible";
    if (major >= 1) return "incompatible";
    return "unknown";
  }

  if (platform === "macos") {
    const [major, minor] = parts;
    if (major > 15) return "compatible";
    if (major < 15) return "incompatible";
    if (minor === undefined) return "unknown";
    return minor >= 2 ? "compatible" : "incompatible";
  }

  // Chromium deliberately exposes no useful Linux distribution/version value.
  return "unknown";
}

function architectureFromLegacySignals(
  platform: SupportedPlatform,
  navigatorLike: NavigatorLike,
): CpuArchitecture {
  const userAgent = navigatorLike.userAgent ?? "";

  if (/arm64|aarch64/i.test(userAgent)) return "arm64";

  // Safari deliberately exposes "Intel" on Apple Silicon too, so that token is
  // never sufficient to choose a macOS package.
  if (platform === "macos") return "unknown";

  if (/x86_64|x86-64|amd64|win64|wow64|\bx64\b/i.test(userAgent)) return "x64";
  if (/\bi[3-6]86\b/i.test(userAgent)) return "x86";
  return "unknown";
}

export function detectPlatformSync(navigatorLike: NavigatorLike | null): PlatformDetection {
  if (!navigatorLike) return unknownDetection;

  const baseDetection = classifyBasePlatform(navigatorLike);
  if (!baseDetection.platform || baseDetection.deviceKind !== "desktop") {
    return baseDetection;
  }

  const architecture = navigatorLike.userAgentData
    ? "unknown"
    : architectureFromLegacySignals(baseDetection.platform, navigatorLike);
  return {
    ...baseDetection,
    architecture,
    architectureConfidence: architecture === "unknown" ? "low" : "medium",
  };
}

export async function detectPlatform(
  navigatorLike: NavigatorLike | null =
    typeof navigator === "undefined"
      ? null
      : (navigator as Navigator & { userAgentData?: UserAgentDataLike }),
): Promise<PlatformDetection> {
  const fallback = detectPlatformSync(navigatorLike);
  const userAgentData = navigatorLike?.userAgentData;

  if (
    !fallback.platform ||
    fallback.deviceKind !== "desktop" ||
    typeof userAgentData?.getHighEntropyValues !== "function"
  ) {
    return fallback;
  }

  try {
    const values = await userAgentData.getHighEntropyValues([
      "architecture",
      "bitness",
      "platformVersion",
      "wow64",
    ]);
    const architecture = architectureFromClientHints(values);
    const platformVersion = values.platformVersion?.trim() || null;
    const osCompatibility = compatibilityFromPlatformVersion(
      fallback.platform,
      platformVersion ?? undefined,
    );

    return {
      ...fallback,
      architecture,
      architectureConfidence: architecture === "unknown" ? "low" : "high",
      platformVersion,
      osCompatibility,
      source: "ua-client-hints",
    };
  } catch {
    return fallback;
  }
}

export function getCompatibleDownloadTarget(
  detection: PlatformDetection,
): DownloadTarget | null {
  if (
    detection.deviceKind !== "desktop" ||
    detection.hasConflict ||
    detection.platformConfidence === "low"
  ) {
    return null;
  }

  if (
    (detection.platform === "macos" || detection.platform === "windows") &&
    detection.osCompatibility !== "compatible"
  ) {
    return null;
  }

  if (detection.platform === "macos") {
    if (detection.architecture === "arm64") return "macos-arm64";
    if (detection.architecture === "x64") return "macos-x64";
    return null;
  }

  if (detection.platform === "windows") {
    return detection.architecture === "x64" ? "windows-x64" : null;
  }

  if (detection.platform === "linux") {
    return detection.architecture === "x64" ? "linux-x64" : null;
  }

  return null;
}
