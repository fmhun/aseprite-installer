import { describe, expect, it, vi } from "vitest";
import {
  detectPlatform,
  detectPlatformSync,
  getCompatibleDownloadTarget,
  type NavigatorLike,
} from "./platformDetection";

const windowsUa =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140.0.0.0 Safari/537.36";
const macUa =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.6 Safari/605.1.15";
const linuxUa =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/140.0.0.0 Safari/537.36";

describe("platform detection", () => {
  it("detects Windows x64 with high-entropy client hints", async () => {
    const getHighEntropyValues = vi.fn().mockResolvedValue({
      architecture: "x86",
      bitness: "64",
      platformVersion: "13.0.0",
    });
    const detection = await detectPlatform({
      userAgent: windowsUa,
      platform: "Win32",
      maxTouchPoints: 0,
      userAgentData: { platform: "Windows", getHighEntropyValues },
    });

    expect(getHighEntropyValues).toHaveBeenCalledWith([
      "architecture",
      "bitness",
      "platformVersion",
      "wow64",
    ]);
    expect(detection).toMatchObject({
      platform: "windows",
      architecture: "x64",
      platformConfidence: "high",
      architectureConfidence: "high",
      platformVersion: "13.0.0",
      osCompatibility: "compatible",
      deviceKind: "desktop",
      source: "ua-client-hints",
    });
    expect(getCompatibleDownloadTarget(detection)).toBe("windows-x64");
  });

  it.each([
    ["arm", "64", "arm64", "macos-arm64"],
    ["x86", "64", "x64", "macos-x64"],
  ] as const)(
    "uses client hints to distinguish a %s macOS browser",
    async (architecture, bitness, expectedArchitecture, expectedTarget) => {
      const detection = await detectPlatform({
        userAgent: macUa,
        platform: "MacIntel",
        maxTouchPoints: 0,
        userAgentData: {
          platform: "macOS",
          getHighEntropyValues: vi.fn().mockResolvedValue({
            architecture,
            bitness,
            platformVersion: "15.2.0",
          }),
        },
      });

      expect(detection.architecture).toBe(expectedArchitecture);
      expect(getCompatibleDownloadTarget(detection)).toBe(expectedTarget);
    },
  );

  it.each(["Intel Mac", "Apple Silicon Mac"])(
    "does not guess an architecture from identical Safari signals on %s",
    async () => {
      const detection = await detectPlatform({
        userAgent: macUa,
        platform: "MacIntel",
        maxTouchPoints: 0,
      });

      expect(detection).toMatchObject({
        platform: "macos",
        architecture: "unknown",
        platformConfidence: "medium",
        deviceKind: "desktop",
      });
      expect(getCompatibleDownloadTarget(detection)).toBeNull();
    },
  );

  it("excludes iPadOS desktop mode even when it identifies as MacIntel", async () => {
    const detection = await detectPlatform({
      userAgent: macUa,
      platform: "MacIntel",
      maxTouchPoints: 5,
    });

    expect(detection).toMatchObject({
      platform: null,
      architecture: "unknown",
      deviceKind: "mobile",
      exclusionReason: "ipados",
    });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("excludes Android before the generic Linux fallback", () => {
    const detection = detectPlatformSync({
      userAgent:
        "Mozilla/5.0 (Linux; Android 16; Pixel 10) AppleWebKit/537.36 Chrome/140 Mobile Safari/537.36",
      platform: "Linux armv81",
      maxTouchPoints: 5,
      userAgentData: { platform: "Android" },
    });

    expect(detection.platform).toBeNull();
    expect(detection.deviceKind).toBe("mobile");
    expect(detection.exclusionReason).toBe("android");
  });

  it("excludes ChromeOS before the reduced Linux platform value", () => {
    const detection = detectPlatformSync({
      userAgent:
        "Mozilla/5.0 (X11; CrOS x86_64 16000.0.0) AppleWebKit/537.36 Chrome/140 Safari/537.36",
      platform: "Linux x86_64",
      maxTouchPoints: 0,
      userAgentData: { platform: "Chrome OS" },
    });

    expect(detection.platform).toBeNull();
    expect(detection.deviceKind).toBe("unsupported");
    expect(detection.exclusionReason).toBe("chromeos");
  });

  it("recommends AppImage for a legacy Linux x86_64 browser", async () => {
    const detection = await detectPlatform({
      userAgent: linuxUa,
      platform: "Linux x86_64",
      maxTouchPoints: 0,
    });

    expect(detection).toMatchObject({
      platform: "linux",
      architecture: "x64",
      platformConfidence: "medium",
      architectureConfidence: "medium",
    });
    expect(getCompatibleDownloadTarget(detection)).toBe("linux-x64");
  });

  it("selects Linux but does not offer the x86_64 package on ARM64", async () => {
    const detection = await detectPlatform({
      userAgent: "Mozilla/5.0 (X11; Linux aarch64) Gecko/20100101 Firefox/141.0",
      platform: "Linux aarch64",
      maxTouchPoints: 0,
    });

    expect(detection).toMatchObject({ platform: "linux", architecture: "arm64" });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("selects Windows but does not claim x64 compatibility on ARM64", async () => {
    const detection = await detectPlatform({
      userAgent: windowsUa,
      platform: "Win32",
      maxTouchPoints: 0,
      userAgentData: {
        platform: "Windows",
        getHighEntropyValues: vi.fn().mockResolvedValue({
          architecture: "arm",
          bitness: "64",
          platformVersion: "13.0.0",
        }),
      },
    });

    expect(detection).toMatchObject({ platform: "windows", architecture: "arm64" });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("does not trust a frozen Chromium architecture when high-entropy hints are refused", async () => {
    const detection = await detectPlatform({
      userAgent: windowsUa,
      platform: "Win32",
      maxTouchPoints: 0,
      userAgentData: {
        platform: "Windows",
        getHighEntropyValues: vi.fn().mockRejectedValue(new DOMException("Denied")),
      },
    });

    expect(detection).toMatchObject({
      platform: "windows",
      architecture: "unknown",
      architectureConfidence: "low",
    });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("keeps architecture unknown when client hints return no usable value", async () => {
    const detection = await detectPlatform({
      userAgent: windowsUa,
      platform: "Win32",
      userAgentData: {
        platform: "Windows",
        getHighEntropyValues: vi.fn().mockResolvedValue({}),
      },
    });

    expect(detection).toMatchObject({ platform: "windows", architecture: "unknown" });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("keeps architecture unknown when UA-CH exists without high-entropy access", async () => {
    const detection = await detectPlatform({
      userAgent: windowsUa,
      platform: "Win32",
      userAgentData: { platform: "Windows", mobile: false },
    });

    expect(detection).toMatchObject({
      platform: "windows",
      architecture: "unknown",
      platformConfidence: "high",
    });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("recognizes a 64-bit Windows host exposed through WOW64", async () => {
    const detection = await detectPlatform({
      userAgent: windowsUa,
      platform: "Win32",
      userAgentData: {
        platform: "Windows",
        getHighEntropyValues: vi.fn().mockResolvedValue({
          architecture: "x86",
          bitness: "32",
          platformVersion: "13.0.0",
          wow64: true,
        }),
      },
    });

    expect(detection.architecture).toBe("x64");
    expect(getCompatibleDownloadTarget(detection)).toBe("windows-x64");
  });

  it("rejects a desktop platform when UA-CH marks the browser as mobile", async () => {
    const detection = await detectPlatform({
      userAgent: macUa,
      platform: "MacIntel",
      maxTouchPoints: 0,
      userAgentData: { platform: "macOS", mobile: true },
    });

    expect(detection).toMatchObject({
      platform: null,
      deviceKind: "mobile",
      exclusionReason: "other-mobile",
    });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("recognizes iPadOS from a Mac-like UA-CH signal and touch input", async () => {
    const detection = await detectPlatform({
      userAgent: "Mozilla/5.0 AppleWebKit/605.1.15 Safari/604.1",
      platform: "",
      maxTouchPoints: 5,
      userAgentData: { platform: "macOS", mobile: false },
    });

    expect(detection).toMatchObject({
      platform: null,
      deviceKind: "mobile",
      exclusionReason: "ipados",
      source: "ua-client-hints",
    });
  });

  it("recognizes explicit iOS signals", () => {
    const detection = detectPlatformSync({
      userAgent:
        "Mozilla/5.0 (iPhone; CPU iPhone OS 19_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148 Safari/604.1",
      platform: "iPhone",
      maxTouchPoints: 5,
    });

    expect(detection).toMatchObject({
      platform: null,
      deviceKind: "mobile",
      exclusionReason: "ios",
    });
  });

  it("does not infer x64 from Win32 alone", async () => {
    const detection = await detectPlatform({
      userAgent: "Mozilla/5.0 (Windows NT 10.0) Gecko/20100101 Firefox/141.0",
      platform: "Win32",
    });

    expect(detection).toMatchObject({ platform: "windows", architecture: "unknown" });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("does not equate an arbitrary X11 Unix browser with Linux", () => {
    const detection = detectPlatformSync({
      userAgent: "Mozilla/5.0 (X11; FreeBSD amd64) Gecko/20100101 Firefox/141.0",
      platform: "FreeBSD amd64",
    });

    expect(detection.platform).toBeNull();
    expect(detection.deviceKind).toBe("unknown");
  });

  it("refuses a direct package when platform signals conflict", async () => {
    const navigatorLike: NavigatorLike = {
      userAgent: linuxUa,
      platform: "Linux x86_64",
      maxTouchPoints: 0,
      userAgentData: {
        platform: "Windows",
        getHighEntropyValues: vi.fn().mockResolvedValue({
          architecture: "x86",
          bitness: "64",
          platformVersion: "13.0.0",
        }),
      },
    };
    const detection = await detectPlatform(navigatorLike);

    expect(detection).toMatchObject({
      platform: "windows",
      architecture: "x64",
      platformConfidence: "low",
    });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("returns an unknown fallback when the navigator exposes no useful signal", async () => {
    const detection = await detectPlatform({ userAgent: "", platform: "" });

    expect(detection).toMatchObject({
      platform: null,
      architecture: "unknown",
      deviceKind: "unknown",
      source: "none",
    });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("blocks the Windows package when client hints identify Windows 10", async () => {
    const detection = await detectPlatform({
      userAgent: windowsUa,
      platform: "Win32",
      userAgentData: {
        platform: "Windows",
        getHighEntropyValues: vi.fn().mockResolvedValue({
          architecture: "x86",
          bitness: "64",
          platformVersion: "10.0.0",
        }),
      },
    });

    expect(detection).toMatchObject({
      platform: "windows",
      architecture: "x64",
      platformVersion: "10.0.0",
      osCompatibility: "incompatible",
    });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });

  it("blocks a macOS version below the 15.2 minimum", async () => {
    const detection = await detectPlatform({
      userAgent: macUa,
      platform: "MacIntel",
      userAgentData: {
        platform: "macOS",
        getHighEntropyValues: vi.fn().mockResolvedValue({
          architecture: "arm",
          bitness: "64",
          platformVersion: "15.1.9",
        }),
      },
    });

    expect(detection).toMatchObject({
      platform: "macos",
      architecture: "arm64",
      platformVersion: "15.1.9",
      osCompatibility: "incompatible",
    });
    expect(getCompatibleDownloadTarget(detection)).toBeNull();
  });
});
