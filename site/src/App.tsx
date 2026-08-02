import {
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import installerIcon from "../../assets/icons/aseprite-installer.svg";
import localBuildIcon from "../../assets/icons/aseprite-local.svg";
import type { DemoPhase } from "./demo";
import {
  detectPlatform,
  detectPlatformSync,
  getCompatibleDownloadTarget,
  supportedPlatforms,
  type DownloadTarget,
  type NavigatorLike,
  type PlatformDetection,
  type SupportedPlatform,
} from "./platformDetection";
import {
  getPlatformSimulationServerSnapshot,
  getPlatformSimulationSnapshot,
  getSimulatedPlatformDetection,
  subscribePlatformSimulation,
} from "./platformSimulation";
import { useDemoPlayback } from "./useDemoPlayback";

const GITHUB_URL = "https://github.com/fmhun/aseprite-installer";
const DOWNLOAD_URL = `${GITHUB_URL}/releases/latest`;
const DOWNLOAD_ASSET_URL = `${DOWNLOAD_URL}/download`;
const BUY_URL = "https://www.aseprite.org/buy/";
const EULA_URL = "https://github.com/aseprite/aseprite/blob/main/EULA.txt";

const downloads = {
  macosArmDmg: `${DOWNLOAD_ASSET_URL}/Aseprite-Installer-macOS-arm64.dmg`,
  macosIntelDmg: `${DOWNLOAD_ASSET_URL}/Aseprite-Installer-macOS-x64.dmg`,
  macosArmArchive: `${DOWNLOAD_ASSET_URL}/Aseprite-Installer-macOS-arm64.app.zip`,
  macosIntelArchive: `${DOWNLOAD_ASSET_URL}/Aseprite-Installer-macOS-x64.app.zip`,
  windowsNsis: `${DOWNLOAD_ASSET_URL}/Aseprite-Installer-Windows-x64-setup.exe`,
  windowsMsi: `${DOWNLOAD_ASSET_URL}/Aseprite-Installer-Windows-x64.msi`,
  linuxAppImage: `${DOWNLOAD_ASSET_URL}/Aseprite-Installer-Linux-x86_64.AppImage`,
  linuxDeb: `${DOWNLOAD_ASSET_URL}/Aseprite-Installer-Linux-x86_64.deb`,
  linuxRpm: `${DOWNLOAD_ASSET_URL}/Aseprite-Installer-Linux-x86_64.rpm`,
  checksums: `${DOWNLOAD_ASSET_URL}/SHA256SUMS`,
} as const;

const directDownloadCtas: Record<
  DownloadTarget,
  { href: string; label: string; platform: SupportedPlatform; status: string }
> = {
  "macos-arm64": {
    href: downloads.macosArmDmg,
    label: "Download for Apple Silicon",
    platform: "macos",
    status: "Apple Silicon detected · Apple Silicon DMG recommended",
  },
  "macos-x64": {
    href: downloads.macosIntelDmg,
    label: "Download for Intel Mac",
    platform: "macos",
    status: "Intel Mac detected · Intel DMG recommended",
  },
  "windows-x64": {
    href: downloads.windowsNsis,
    label: "Download for Windows",
    platform: "windows",
    status: "Windows x64 detected · NSIS installer recommended",
  },
  "linux-x64": {
    href: downloads.linuxAppImage,
    label: "Download AppImage",
    platform: "linux",
    status: "Linux x86_64 detected · AppImage recommended",
  },
};

type LinuxRecipeId = "appimage" | "deb" | "rpm" | "zypper";

const linuxDownloadDirectoryCommand =
  'ASEPRITE_DOWNLOAD_DIR="$(xdg-user-dir DOWNLOAD 2>/dev/null)"; ASEPRITE_DOWNLOAD_DIR="${ASEPRITE_DOWNLOAD_DIR:-$HOME/Downloads}"';

function verifiedLinuxCommand(asset: string, installCommand: string): string {
  const assetUrl = `${DOWNLOAD_ASSET_URL}/${asset}`;
  const curlDownloads = `curl -fL --retry 3 -o "${asset}" "${assetUrl}" && curl -fL --retry 3 -o SHA256SUMS "${downloads.checksums}"`;
  const wgetDownloads = `wget -O "${asset}" "${assetUrl}" && wget -O SHA256SUMS "${downloads.checksums}"`;

  return `${linuxDownloadDirectoryCommand}; mkdir -p "$ASEPRITE_DOWNLOAD_DIR" && cd "$ASEPRITE_DOWNLOAD_DIR" && ( (${curlDownloads}) || (${wgetDownloads}) ) && grep -F "  ${asset}" SHA256SUMS > "${asset}.sha256" && sha256sum --check "${asset}.sha256" && rm -f "${asset}.sha256" && ${installCommand}`;
}

const linuxInstallRecipes: Record<
  LinuxRecipeId,
  {
    command: string;
    detail: string;
    label: string;
    note: string;
  }
> = {
  appimage: {
    label: "AppImage",
    detail: "Portable · no administrator access",
    command: verifiedLinuxCommand(
      "Aseprite-Installer-Linux-x86_64.AppImage",
      'chmod u+x "Aseprite-Installer-Linux-x86_64.AppImage" && "./Aseprite-Installer-Linux-x86_64.AppImage"',
    ),
    note: "Downloads and verifies the AppImage, makes it executable, then launches it. Nothing is installed system-wide.",
  },
  deb: {
    label: "Debian / Ubuntu",
    detail: "Debian or Ubuntu · uses apt",
    command: verifiedLinuxCommand(
      "Aseprite-Installer-Linux-x86_64.deb",
      'sudo apt install "./Aseprite-Installer-Linux-x86_64.deb"',
    ),
    note: "Downloads and verifies the package, then apt installs it and resolves its dependencies. Your system may ask for your password.",
  },
  rpm: {
    label: "Fedora / RHEL",
    detail: "Fedora or compatible · uses dnf",
    command: verifiedLinuxCommand(
      "Aseprite-Installer-Linux-x86_64.rpm",
      'sudo dnf install "./Aseprite-Installer-Linux-x86_64.rpm"',
    ),
    note: "Downloads and verifies the package, then dnf installs it and resolves its dependencies. Your system may ask for your password.",
  },
  zypper: {
    label: "openSUSE",
    detail: "openSUSE · uses zypper",
    command: verifiedLinuxCommand(
      "Aseprite-Installer-Linux-x86_64.rpm",
      'sudo zypper install "./Aseprite-Installer-Linux-x86_64.rpm"',
    ),
    note: "Downloads and verifies the package, then zypper installs it and resolves its dependencies. Your system may ask for your password.",
  },
};

function ExternalArrow() {
  return <span aria-hidden="true">↗</span>;
}

function PixelButton({ children, tone = "primary" }: { children: ReactNode; tone?: "primary" | "secondary" }) {
  return <div className={`demo-button demo-button--${tone}`}>{children}</div>;
}

function DemoStepper({ current }: { current: number }) {
  const steps = ["Release", "Tools", "Install"];
  return (
    <div className="demo-stepper">
      {steps.map((label, index) => (
        <div
          className={index < current ? "demo-step demo-step--done" : index === current ? "demo-step demo-step--current" : "demo-step"}
          key={label}
        >
          <span>{index < current ? "✓" : index + 1}</span>
          <small>{label}</small>
        </div>
      ))}
    </div>
  );
}

function StatusScreen() {
  return (
    <div className="demo-panel demo-status">
      <div className="demo-status-mark">+</div>
      <div>
        <p className="demo-panel-title">No local copy found</p>
        <p>Start with an official release.</p>
      </div>
      <div className="demo-choice"><span>PERSONAL SOURCE BUILD</span></div>
      <PixelButton tone="secondary">Compile a personal copy →</PixelButton>
    </div>
  );
}

function ReleaseScreen() {
  return (
    <>
      <DemoStepper current={0} />
      <div className="demo-panel demo-flow-panel">
        <div className="demo-kicker">STEP 1 OF 3</div>
        <p className="demo-panel-title">Choose a release</p>
        <p>Only official source archives are listed.</p>
        <div className="demo-field-label">ASEPRITE RELEASE</div>
        <div className="demo-select">
          <span>Latest stable — verified</span>
          <span>⌄</span>
        </div>
        <div className="demo-release-meta">
          <span>STABLE</span><span>SHA-256 ✓</span>
        </div>
        <PixelButton>Continue to checks →</PixelButton>
      </div>
    </>
  );
}

const tools = [
  ["C++ toolchain", "Detected"],
  ["CMake", "Ready"],
  ["Ninja", "Ready"],
  ["Free space", "6 GB+"],
] as const;

function PreflightScreen() {
  return (
    <>
      <DemoStepper current={1} />
      <div className="demo-panel demo-flow-panel">
        <div className="demo-kicker">STEP 2 OF 3</div>
        <p className="demo-panel-title">Ready to build</p>
        <p>Your build tools stay on this device.</p>
        <ul className="demo-tool-list">
          {tools.map(([tool, version]) => (
            <li key={tool}>
              <span className="demo-check">✓</span>
              <strong>{tool}</strong>
              <small>{version}</small>
            </li>
          ))}
        </ul>
        <PixelButton>Install →</PixelButton>
      </div>
    </>
  );
}

const stageCopy = {
  download: ["Downloading official source", "github.com/aseprite/aseprite"],
  verify: ["Verifying source archive", "SHA-256 digest matched"],
  compile: ["Compiling your local app", "isolated CMake + Ninja build"],
  validate: ["Validating local artifact", "native executable checked"],
  install: ["Installing safely", "staging before replacement"],
} as const;

function BuildScreen({ progress, buildStage }: { progress: number; buildStage: keyof typeof stageCopy }) {
  const [title, detail] = stageCopy[buildStage];
  return (
    <>
      <DemoStepper current={2} />
      <div className="demo-panel demo-build-panel">
        <div className="demo-kicker">STEP 3 OF 3</div>
        <div className="demo-build-symbol">
          <i /><i /><i /><i />
        </div>
        <p className="demo-panel-title">{title}</p>
        <p>{detail}</p>
        <div className="demo-progress-copy"><span>{buildStage.toUpperCase()}</span><strong>{progress}%</strong></div>
        <div className="demo-progress-track"><span style={{ width: `${progress}%` }} /></div>
        <div className="demo-terminal">
          <span>$ verified personal build</span>
          <span className="demo-terminal-active">› {title.toLowerCase()}…</span>
        </div>
      </div>
    </>
  );
}

function CompleteScreen() {
  return (
    <>
      <DemoStepper current={3} />
      <div className="demo-panel demo-status demo-complete">
        <img src={localBuildIcon} alt="" width="58" height="58" />
        <div className="demo-status-mark demo-status-mark--success">✓</div>
        <div>
          <div className="demo-kicker">BUILD COMPLETE</div>
          <p className="demo-panel-title">Your local app is ready</p>
          <p>Aseprite · personal build</p>
          <code>Managed copy installed</code>
        </div>
        <PixelButton>Done</PixelButton>
      </div>
    </>
  );
}

function ScreenForPhase({ phase, progress, buildStage }: { phase: DemoPhase; progress: number; buildStage: keyof typeof stageCopy }) {
  if (phase === "status") return <StatusScreen />;
  if (phase === "release") return <ReleaseScreen />;
  if (phase === "preflight") return <PreflightScreen />;
  if (phase === "build") return <BuildScreen progress={progress} buildStage={buildStage} />;
  return <CompleteScreen />;
}

export function ProductDemo() {
  const demoRef = useRef<HTMLElement>(null);
  const frame = useDemoPlayback(demoRef);

  return (
    <figure
      className="demo-monitor"
      ref={demoRef}
      data-phase={frame.phase}
      data-playing={frame.isPlaying}
      data-reduced-motion={frame.reducedMotion}
      aria-labelledby="demo-caption"
    >
      <div className="demo-bezel" aria-hidden="true">
        <div className="demo-camera" />
        <div className="demo-screen">
          <div className="demo-appbar">
            <div className="demo-window-controls"><i>—</i><i>□</i><i>×</i></div>
            <div className="demo-brand">
              <img src={installerIcon} alt="" width="26" height="26" />
              <span>Aseprite Installer</span>
            </div>
            <span className="demo-app-version">v0.2</span>
          </div>
          <div className="demo-workspace">
            <ScreenForPhase phase={frame.phase} progress={frame.progress} buildStage={frame.buildStage} />
          </div>
          <div className={`demo-cursor ${frame.clicking ? "demo-cursor--clicking" : ""}`}>
            <span />
          </div>
        </div>
        <div className="demo-bezel-label">LOCAL BUILD STATION</div>
      </div>
      <div className="demo-stand" aria-hidden="true"><span /></div>
      <figcaption id="demo-caption" className="site-sr-only">
        The installer selects an official Aseprite release, checks native build tools, verifies and compiles the source, then installs a managed personal copy on your system.
      </figcaption>
    </figure>
  );
}

const platformIds = supportedPlatforms;
type PlatformId = SupportedPlatform;

const platformLabels: Record<PlatformId, string> = {
  macos: "macOS",
  windows: "Windows",
  linux: "Linux",
};

interface SmartDownloadCta {
  href: string;
  label: string;
  target: DownloadTarget | null;
}

interface CtaSelection {
  detection: PlatformDetection | null;
  manualPlatform: PlatformId | null;
}

function getSmartDownloadCta(
  detection: PlatformDetection | null,
  manualPlatform: PlatformId | null,
): SmartDownloadCta {
  const detectedTarget = detection ? getCompatibleDownloadTarget(detection) : null;
  const directCta = detectedTarget ? directDownloadCtas[detectedTarget] : null;

  if (manualPlatform) {
    if (directCta?.platform === manualPlatform) {
      return { href: directCta.href, label: directCta.label, target: detectedTarget };
    }
    return {
      href: "#install",
      label:
        manualPlatform === "macos"
          ? "Choose Apple Silicon or Intel"
          : `Choose a ${platformLabels[manualPlatform]} package`,
      target: null,
    };
  }

  if (directCta && detectedTarget) {
    return { href: directCta.href, label: directCta.label, target: detectedTarget };
  }

  if (detection?.platform === "macos" && !detection.hasConflict) {
    return { href: "#install", label: "Choose Apple Silicon or Intel", target: null };
  }
  if (detection?.platform === "windows" && !detection.hasConflict) {
    return { href: "#install", label: "Review Windows downloads", target: null };
  }
  if (detection?.platform === "linux" && !detection.hasConflict) {
    return { href: "#install", label: "Choose a compatible Linux package", target: null };
  }
  return { href: "#install", label: "Choose your platform", target: null };
}

function getDetectionStatus(
  detection: PlatformDetection | null,
  manualPlatform: PlatformId | null,
): string {
  if (manualPlatform) {
    return `${platformLabels[manualPlatform]} selected manually · automatic selection paused`;
  }
  if (!detection) {
    return "Automatic detection runs locally · manual choice always wins";
  }
  if (detection.hasConflict) {
    return "System signals disagree · choose your platform manually";
  }
  if (detection.deviceKind === "mobile") {
    return "Mobile device detected · choose the target desktop computer";
  }
  if (detection.exclusionReason === "chromeos") {
    return "ChromeOS detected · choose a package for another supported computer";
  }
  if (detection.deviceKind === "unsupported") {
    return "Unsupported system detected · choose the target computer manually";
  }

  const target = getCompatibleDownloadTarget(detection);
  if (target) return directDownloadCtas[target].status;

  if (detection.platform === "macos") {
    if (detection.osCompatibility === "incompatible") {
      return `${detection.platformVersion ? `macOS ${detection.platformVersion}` : "This macOS version"} detected · macOS 15.2+ is required`;
    }
    if (detection.architecture !== "unknown" && detection.osCompatibility === "unknown") {
      return "Mac architecture detected · confirm macOS 15.2+ before downloading";
    }
    return "macOS detected · choose Apple Silicon or Intel";
  }
  if (detection.platform === "windows") {
    if (detection.osCompatibility === "incompatible") {
      return "Windows 10 detected · Windows 11 is required";
    }
    return detection.architecture === "arm64"
      ? "Windows ARM64 detected · current packages require x64"
      : detection.architecture === "x64" && detection.osCompatibility === "unknown"
        ? "Windows x64 detected · confirm Windows 11 before downloading"
        : "Windows detected · confirm a Windows 11 x64 system";
  }
  if (detection.platform === "linux") {
    return detection.architecture === "arm64"
      ? "Linux ARM64 detected · current packages require x86_64"
      : "Linux detected · choose a compatible x86_64 package";
  }
  return "System not recognized · choose your platform manually";
}

async function copyTextToClipboard(value: string): Promise<void> {
  if (typeof navigator.clipboard?.writeText === "function") {
    try {
      await navigator.clipboard.writeText(value);
      return;
    } catch {
      // Fall through to the selection-based copy path for restrictive browsers.
    }
  }

  const previousFocus =
    document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const textarea = document.createElement("textarea");
  textarea.value = value;
  textarea.readOnly = true;
  textarea.tabIndex = -1;
  textarea.style.position = "fixed";
  textarea.style.inset = "0 auto auto -9999px";
  let copied = false;
  try {
    document.body.append(textarea);
    textarea.select();
    copied =
      typeof document.execCommand === "function" && document.execCommand("copy");
  } finally {
    textarea.remove();
    previousFocus?.focus();
  }

  if (!copied) throw new Error("Clipboard access is unavailable");
}

function LinuxQuickInstall({
  activeRecipe,
  onRecipeChange,
}: {
  activeRecipe: LinuxRecipeId;
  onRecipeChange: (recipe: LinuxRecipeId) => void;
}) {
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const copyOperationRef = useRef(0);
  const resetTimerRef = useRef<number | null>(null);
  const recipe = linuxInstallRecipes[activeRecipe];

  useEffect(() => () => {
    copyOperationRef.current += 1;
    if (resetTimerRef.current !== null) window.clearTimeout(resetTimerRef.current);
  }, []);

  const chooseRecipe = (nextRecipe: LinuxRecipeId) => {
    copyOperationRef.current += 1;
    if (resetTimerRef.current !== null) window.clearTimeout(resetTimerRef.current);
    resetTimerRef.current = null;
    setCopyState("idle");
    onRecipeChange(nextRecipe);
  };

  const copyCommand = async () => {
    const operation = copyOperationRef.current + 1;
    copyOperationRef.current = operation;
    try {
      await copyTextToClipboard(recipe.command);
      if (copyOperationRef.current !== operation) return;
      setCopyState("copied");
    } catch {
      if (copyOperationRef.current !== operation) return;
      setCopyState("failed");
    }

    if (resetTimerRef.current !== null) window.clearTimeout(resetTimerRef.current);
    resetTimerRef.current = window.setTimeout(() => {
      resetTimerRef.current = null;
      setCopyState("idle");
    }, 2_000);
  };

  const copyLabel =
    copyState === "copied" ? "Copied!" : copyState === "failed" ? "Copy failed" : "Copy";

  return (
    <section className="site-quick-install" aria-labelledby="linux-quick-install-title">
      <div className="site-quick-install-heading">
        <div>
          <span className="site-terminal-kicker">QUICK INSTALL</span>
          <h4 id="linux-quick-install-title">One command. Verified install.</h4>
        </div>
        <p>Pick your system, copy once, then paste into Terminal. Download and SHA-256 verification are automatic.</p>
      </div>

      <div className="site-recipe-switcher" role="group" aria-label="Linux install recipe">
        {(Object.keys(linuxInstallRecipes) as LinuxRecipeId[]).map((recipeId) => (
          <button
            aria-pressed={activeRecipe === recipeId}
            key={recipeId}
            onClick={() => chooseRecipe(recipeId)}
            type="button"
          >
            {linuxInstallRecipes[recipeId].label}
          </button>
        ))}
      </div>

      <div className="site-terminal-window">
        <div className="site-terminal-titlebar">
          <span aria-hidden="true"><i /><i /><i /></span>
          <strong>TERMINAL · {recipe.label.toUpperCase()}</strong>
          <small>{recipe.detail}</small>
        </div>
        <div className="site-terminal-content">
          <div className="site-command-step">
            <span className="site-command-step-label"><b>01</b> Copy, paste, and run</span>
            <div className="site-command-line">
              <span className="site-command-prompt" aria-hidden="true">$</span>
              <pre tabIndex={0}><code>{recipe.command}</code></pre>
              <button
                aria-label={`Copy ${recipe.label} install command`}
                className={`site-copy-command${copyState === "copied" ? " is-copied" : ""}`}
                onClick={() => { void copyCommand(); }}
                type="button"
              >
                <span aria-hidden="true">▣</span> {copyLabel}
              </button>
            </div>
          </div>
          <p className="site-command-note">{recipe.note}</p>
          <p className="site-command-assumption">
            Saves to your Linux Downloads folder automatically, with <code>~/Downloads</code> as fallback.
            Requires either <code>curl</code> or <code>wget</code> and <code>sha256sum</code>.
          </p>
          <span className="site-sr-only" role="status" aria-live="polite">
            {copyState === "copied"
              ? `${recipe.label} install command copied to clipboard.`
              : copyState === "failed"
                ? "Automatic copy failed. Select the command and copy it manually."
                : ""}
          </span>
        </div>
      </div>
    </section>
  );
}

function PackageLink({
  href,
  label,
  detail,
  tone = "secondary",
  onActivate,
}: {
  href: string;
  label: string;
  detail: string;
  tone?: "primary" | "secondary";
  onActivate?: () => void;
}) {
  return (
    <a
      className={`site-package-link site-package-link--${tone}`}
      href={href}
      onClick={onActivate}
    >
      <span><strong>{label}</strong><small>{detail}</small></span>
      <span aria-hidden="true">↓</span>
    </a>
  );
}

function DownloadPicker({
  activePlatform,
  detectionStatus,
  onPlatformChange,
}: {
  activePlatform: PlatformId | null;
  detectionStatus: string;
  onPlatformChange: (platform: PlatformId) => void;
}) {
  const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [activeLinuxRecipe, setActiveLinuxRecipe] = useState<LinuxRecipeId>("appimage");

  const activateTab = (index: number) => {
    const platform = platformIds[index];
    onPlatformChange(platform);
    tabRefs.current[index]?.focus();
  };

  const handleTabKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let nextIndex: number | undefined;

    if (event.key === "ArrowRight") nextIndex = (index + 1) % platformIds.length;
    if (event.key === "ArrowLeft") nextIndex = (index - 1 + platformIds.length) % platformIds.length;
    if (event.key === "Home") nextIndex = 0;
    if (event.key === "End") nextIndex = platformIds.length - 1;

    if (nextIndex !== undefined) {
      event.preventDefault();
      activateTab(nextIndex);
    }
  };

  return (
    <div className="site-download-picker" data-active-platform={activePlatform ?? "none"}>
      <p className="site-detection-note" role="status">
        <span aria-hidden="true">◆</span> {detectionStatus}
      </p>
      <div className="site-platform-tabs" role="tablist" aria-label="Choose your operating system">
        {platformIds.map((platform, index) => (
          <button
            className="site-platform-tab"
            id={`platform-${platform}-tab`}
            key={platform}
            type="button"
            role="tab"
            aria-controls={`platform-${platform}-panel`}
            aria-selected={activePlatform === platform}
            tabIndex={activePlatform === platform || (!activePlatform && index === 0) ? 0 : -1}
            ref={(node) => { tabRefs.current[index] = node; }}
            onClick={() => onPlatformChange(platform)}
            onKeyDown={(event) => handleTabKeyDown(event, index)}
          >
            {platformLabels[platform]}
          </button>
        ))}
      </div>

      {!activePlatform && (
        <div className="site-platform-prompt">
          <strong>Choose the desktop you want to install on.</strong>
          <p>Pick macOS, Windows, or Linux above to see its verified packages and requirements.</p>
        </div>
      )}

      <section
        className="site-platform-panel"
        id="platform-macos-panel"
        role="tabpanel"
        aria-labelledby="platform-macos-tab"
        hidden={activePlatform !== "macos"}
        tabIndex={activePlatform === "macos" ? 0 : -1}
      >
        <div className="site-platform-meta">
          <div><h3>macOS</h3><p>macOS 15.2+ · Apple Silicon or Intel</p></div>
          <span>AD-HOC SIGNED</span>
        </div>
        <p>Choose the DMG that matches your Mac. App archives are also available for manual deployment.</p>
        <div className="site-platform-packages">
          <PackageLink href={downloads.macosArmDmg} label="Apple Silicon DMG" detail="Recommended for M-series Macs · arm64" tone="primary" />
          <PackageLink href={downloads.macosIntelDmg} label="Intel DMG" detail="For Intel-based Macs · x64" tone="primary" />
          <PackageLink href={downloads.macosArmArchive} label="Apple Silicon app archive" detail="Manual deployment · arm64 .app.zip" />
          <PackageLink href={downloads.macosIntelArchive} label="Intel app archive" detail="Manual deployment · x64 .app.zip" />
        </div>
        <p className="site-platform-install"><strong>Install:</strong> Download → verify → open the DMG → move the app to Applications, or run it from the DMG.</p>
        <div className="site-platform-requirements">
          <strong>Before you build</strong>
          <ul>
            <li>Xcode with the macOS SDK and command-line tools</li>
            <li>CMake 3.20+ and Ninja 1.10+</li>
            <li>About 6 GB of free temporary space</li>
          </ul>
        </div>
        <p className="site-platform-guidance">With your approval, Aseprite Installer can set up CMake and Ninja through an existing safe Homebrew installation. It does not install Homebrew or Xcode.</p>
        <p className="site-platform-warning"><strong>First launch:</strong> the app is ad-hoc signed, not notarized. Verify its checksum, then Control-click the app and choose <strong>Open</strong> if Gatekeeper asks.</p>
      </section>

      <section
        className="site-platform-panel"
        id="platform-windows-panel"
        role="tabpanel"
        aria-labelledby="platform-windows-tab"
        hidden={activePlatform !== "windows"}
        tabIndex={activePlatform === "windows" ? 0 : -1}
      >
        <div className="site-platform-meta">
          <div><h3>Windows</h3><p>Windows 11 · x64</p></div>
          <span>UNSIGNED</span>
        </div>
        <p>Use NSIS for a personal installation. Choose MSI only for centrally managed deployment.</p>
        <div className="site-platform-packages">
          <PackageLink href={downloads.windowsNsis} label="NSIS installer" detail="Recommended · current-user .exe" tone="primary" />
          <PackageLink href={downloads.windowsMsi} label="MSI package" detail="Managed deployment · x64 .msi" />
        </div>
        <p className="site-platform-install"><strong>Install:</strong> Download → verify → run the current-user installer. Use MSI only for managed deployment.</p>
        <div className="site-platform-requirements">
          <strong>Before you build</strong>
          <ul>
            <li>Visual Studio 2022 with Desktop development with C++</li>
            <li>x64 MSVC tools and Windows SDK 10.0.26100</li>
            <li>CMake 3.20+, Ninja 1.10+, and about 6 GB free</li>
          </ul>
        </div>
        <p className="site-platform-guidance">Aseprite Installer detects what is missing and guides you through the fix. It never launches Visual Studio Installer or makes system-level changes for you.</p>
        <p className="site-platform-warning"><strong>First launch:</strong> packages are not Authenticode-signed, so SmartScreen may warn. Verify the checksum and provenance before choosing <strong>More info → Run anyway</strong>. Do not install NSIS and MSI side by side.</p>
      </section>

      <section
        className="site-platform-panel"
        id="platform-linux-panel"
        role="tabpanel"
        aria-labelledby="platform-linux-tab"
        hidden={activePlatform !== "linux"}
        tabIndex={activePlatform === "linux" ? 0 : -1}
      >
        <div className="site-platform-meta">
          <div><h3>Linux</h3><p>x86_64 · Ubuntu 22.04 / Debian 12 baseline</p></div>
          <span>UNSIGNED</span>
        </div>
        <p>AppImage is the least invasive choice. Use deb or rpm for native package-manager integration.</p>
        <div className="site-platform-packages">
          <PackageLink href={downloads.linuxAppImage} label="AppImage" detail="Recommended · portable x86_64" tone="primary" onActivate={() => setActiveLinuxRecipe("appimage")} />
          <PackageLink href={downloads.linuxDeb} label="deb package" detail="Debian and Ubuntu · x86_64" onActivate={() => setActiveLinuxRecipe("deb")} />
          <PackageLink href={downloads.linuxRpm} label="rpm package" detail="Fedora and compatible systems · x86_64" onActivate={() => setActiveLinuxRecipe("rpm")} />
        </div>
        <LinuxQuickInstall
          activeRecipe={activeLinuxRecipe}
          onRecipeChange={setActiveLinuxRecipe}
        />
        <p className="site-platform-install"><strong>Install:</strong> choose the recipe above, copy its command, and paste it into Terminal. Download and verification happen before installation.</p>
        <div className="site-platform-requirements">
          <strong>Before you build</strong>
          <ul>
            <li>Clang and Clang++ 12+, CMake 3.20+, and Ninja 1.10+</li>
            <li>X11, Xcursor, XInput, XRandR, OpenGL, and fontconfig development libraries</li>
            <li>WebKitGTK 4.1 runtime dependencies and about 6 GB free</li>
          </ul>
        </div>
        <p className="site-platform-guidance">Aseprite Installer provides the right apt, dnf, pacman, or zypper command for recognized distributions. It never runs <code>sudo</code> or <code>pkexec</code>.</p>
        <p className="site-platform-warning"><strong>First launch:</strong> packages are unsigned. The quick-install command verifies <code>SHA256SUMS</code> and makes AppImage executable before opening it. If you install manually, verify the checksum and provenance yourself.</p>
      </section>

      <div className="site-download-footer">
        <p><strong>Aseprite Installer checks every requirement and gives you the right fix for your system.</strong> System-level changes always remain under your control.</p>
        <div>
          <a href={downloads.checksums}>SHA256SUMS <span aria-hidden="true">↓</span></a>
          <a href={DOWNLOAD_URL}>All release assets <ExternalArrow /></a>
        </div>
      </div>
    </div>
  );
}

function App() {
  const [nativeDetection, setNativeDetection] = useState<PlatformDetection | null>(null);
  const [ctaSelection, setCtaSelection] = useState<CtaSelection>({
    detection: null,
    manualPlatform: null,
  });
  const [manualPlatform, setManualPlatform] = useState<PlatformId | null>(null);
  const simulation = useSyncExternalStore(
    subscribePlatformSimulation,
    getPlatformSimulationSnapshot,
    getPlatformSimulationServerSnapshot,
  );
  const simulatedDetection = getSimulatedPlatformDetection(simulation);
  const detection = simulatedDetection ?? nativeDetection;
  const heroCtaRef = useRef<HTMLAnchorElement>(null);
  const heroCtaPointerActiveRef = useRef(false);
  const pendingCtaSelectionRef = useRef<CtaSelection | null>(null);
  const pendingCtaReleaseTimerRef = useRef<number | null>(null);
  const pointerReleaseTimerRef = useRef<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    const navigatorLike = navigator as Navigator & NavigatorLike;

    setNativeDetection(detectPlatformSync(navigatorLike));
    void detectPlatform(navigatorLike).then((nextDetection) => {
      if (!cancelled) setNativeDetection(nextDetection);
    });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (simulation.id !== null || simulation.revision > 0) {
      setManualPlatform(null);
    }
  }, [simulation.id, simulation.revision]);

  useEffect(() => {
    const nextSelection: CtaSelection = { detection, manualPlatform };
    if (
      heroCtaPointerActiveRef.current ||
      document.activeElement === heroCtaRef.current
    ) {
      pendingCtaSelectionRef.current = nextSelection;
    } else {
      pendingCtaSelectionRef.current = null;
      setCtaSelection(nextSelection);
    }
  }, [detection, manualPlatform]);

  useEffect(() => () => {
    if (pendingCtaReleaseTimerRef.current !== null) {
      window.clearTimeout(pendingCtaReleaseTimerRef.current);
    }
  }, []);

  const activePlatform =
    manualPlatform ??
    (detection === null
      ? "macos"
      : detection.platform && !detection.hasConflict
        ? detection.platform
        : null);
  const smartCta = getSmartDownloadCta(
    ctaSelection.detection,
    ctaSelection.manualPlatform,
  );
  const effectiveDetectionStatus = getDetectionStatus(detection, manualPlatform);
  const detectionStatus = simulation.id
    ? `Simulation · ${effectiveDetectionStatus} · Chrome console override`
    : effectiveDetectionStatus;

  const handlePlatformChoice = (platform: PlatformId) => {
    setManualPlatform(platform);
  };

  const handleHeroCtaBlur = () => {
    heroCtaPointerActiveRef.current = false;
    const pendingSelection = pendingCtaSelectionRef.current;
    if (!pendingSelection) return;
    pendingCtaSelectionRef.current = null;
    setCtaSelection(pendingSelection);
  };

  const handleHeroCtaPointerCancel = () => {
    heroCtaPointerActiveRef.current = false;
    handleHeroCtaBlur();
  };

  const handleHeroCtaClick = () => {
    heroCtaPointerActiveRef.current = false;
    pendingCtaReleaseTimerRef.current = window.setTimeout(() => {
      pendingCtaReleaseTimerRef.current = null;
      if (document.activeElement !== heroCtaRef.current) handleHeroCtaBlur();
    }, 0);
  };

  useEffect(() => {
    const releasePointerGuard = () => {
      if (!heroCtaPointerActiveRef.current) return;
      if (pointerReleaseTimerRef.current !== null) {
        window.clearTimeout(pointerReleaseTimerRef.current);
      }
      pointerReleaseTimerRef.current = window.setTimeout(() => {
        pointerReleaseTimerRef.current = null;
        if (heroCtaPointerActiveRef.current) handleHeroCtaBlur();
      }, 0);
    };

    window.addEventListener("pointerup", releasePointerGuard);
    window.addEventListener("pointercancel", releasePointerGuard);
    return () => {
      window.removeEventListener("pointerup", releasePointerGuard);
      window.removeEventListener("pointercancel", releasePointerGuard);
      if (pointerReleaseTimerRef.current !== null) {
        window.clearTimeout(pointerReleaseTimerRef.current);
      }
    };
  }, []);

  return (
    <div
      className="site-page"
      data-effective-platform={activePlatform ?? "none"}
      data-platform-simulation={simulation.id ?? "none"}
    >
      <a className="site-skip-link" href="#main">Skip to content</a>
      <header className="site-header">
        <a className="site-logo" href="#top" aria-label="Aseprite Installer home">
          <img src={installerIcon} alt="" width="36" height="36" />
          <span>Aseprite Installer</span>
          <small>OPEN SOURCE</small>
        </a>
        <nav className="site-nav" aria-label="Main navigation">
          <a href="#install">Install</a>
          <a href="#how-it-works">How it works</a>
          <a href="#faq">FAQ</a>
          <a href={GITHUB_URL}>GitHub <ExternalArrow /></a>
        </nav>
        <a className="site-button site-button--small" href="#install">Download</a>
      </header>

      <main id="main" tabIndex={-1}>
        <section className="site-hero" id="top">
          <div className="site-hero-copy">
            <h1>Install <em>Aseprite</em><br />from source.</h1>
            <p className="site-eyebrow site-hero-tag"><span /> AND FOR FREE</p>
            <p className="site-lead">
              Aseprite Installer verifies official source, checks your build tools, and compiles a personal copy locally on macOS, Windows, or Linux. The installer is free and MIT-licensed; Aseprite’s EULA still applies.
            </p>
            <div className="site-hero-actions">
              <a
                className="site-button"
                data-download-target={smartCta.target ?? "picker"}
                href={smartCta.href}
                onBlur={handleHeroCtaBlur}
                onClick={handleHeroCtaClick}
                onPointerCancel={handleHeroCtaPointerCancel}
                onPointerDown={() => { heroCtaPointerActiveRef.current = true; }}
                ref={heroCtaRef}
              >
                {smartCta.label} <span aria-hidden="true">↓</span>
              </a>
              <a className="site-text-link" href={GITHUB_URL}>View source on GitHub <ExternalArrow /></a>
            </div>
            <p className="site-compatibility">
              <span>macOS · Apple Silicon + Intel</span><i />
              <span>Windows 11 · x64</span><i />
              <span>Linux x86_64</span>
            </p>
          </div>

          <div className="site-demo-wrap">
            <ProductDemo />
          </div>

          <p className="site-unofficial">
            Unofficial and unaffiliated. This tool does not distribute Aseprite binaries or replace the official paid edition.
          </p>
        </section>

        <section className="site-section site-install" id="install">
          <div className="site-install-copy">
            <p className="site-eyebrow"><span /> INSTALL</p>
            <h2>Choose your platform. Build locally.</h2>
            <p>Select the installer made for your system. Every package comes from the same verified release and builds Aseprite only on your device.</p>
          </div>
          <DownloadPicker
            activePlatform={activePlatform}
            detectionStatus={detectionStatus}
            onPlatformChange={handlePlatformChoice}
          />
        </section>

        <section className="site-section site-how" id="how-it-works">
          <div className="site-section-heading">
            <p className="site-eyebrow"><span /> HOW IT WORKS</p>
            <h2>Three steps.<br />One verified workflow.</h2>
          </div>
          <ol className="site-process">
            <li>
              <div className="site-process-number">01</div>
              <div><h3>Pick a release</h3><p>Choose a stable Aseprite source archive from the official GitHub releases.</p></div>
            </li>
            <li>
              <div className="site-process-number">02</div>
              <div><h3>Prepare your system</h3><p>Check the native toolchain and follow the built-in platform guide for anything missing. System changes remain under your control.</p></div>
            </li>
            <li>
              <div className="site-process-number">03</div>
              <div><h3>Build locally</h3><p>Verify official Aseprite and Skia assets, compile on your device, and stage the result before replacing a managed copy.</p></div>
            </li>
          </ol>
        </section>

        <section className="site-section site-source" id="open-source">
          <div>
            <h2>MIT licensed.<br /><em>OPEN SOURCE</em></h2>
            <p>Aseprite Installer is a transparent Tauri, React, and Rust utility released under the MIT License. Aseprite remains subject to its own EULA. No account, token, analytics, or hidden service.</p>
            <p className="site-source-community">A free tool developed for the Aseprite community.</p>
          </div>
          <nav className="site-source-links" aria-label="Open source project links">
            <a href={GITHUB_URL}><span>Browse the code</span><small>GitHub repository</small><ExternalArrow /></a>
            <a href={`${GITHUB_URL}/releases`}><span>Releases</span><small>Packages, checksums, and changelog</small><ExternalArrow /></a>
            <a href={`${GITHUB_URL}/issues`}><span>Issues</span><small>Report a bug</small><ExternalArrow /></a>
            <a href={`${GITHUB_URL}/security/policy`}><span>Security</span><small>Private reporting</small><ExternalArrow /></a>
          </nav>
        </section>

        <section className="site-section site-faq" id="faq">
          <div className="site-section-heading">
            <p className="site-eyebrow"><span /> FAQ</p>
            <h2>The essentials.</h2>
          </div>
          <div className="site-faq-list">
            <details>
              <summary>Which installer should I choose?<span aria-hidden="true">+</span></summary>
              <p>On macOS, choose the DMG for your Mac’s architecture. On Windows, use the NSIS .exe for a personal installation or MSI for managed deployment. On Linux, AppImage is the least invasive choice; deb and rpm integrate with their matching package managers.</p>
            </details>
            <details>
              <summary>Does the installer distribute Aseprite?<span aria-hidden="true">+</span></summary>
              <p>No. It downloads official Aseprite source and Skia release assets only after you choose a release, verifies their pinned sizes and SHA-256 digests, and compiles your personal copy locally. Aseprite’s EULA still applies.</p>
            </details>
            <details>
              <summary>What happens to an existing copy?<span aria-hidden="true">+</span></summary>
              <p>The new app is staged and validated before replacement. Installer-managed builds can keep a backup; Steam and package-manager copies remain read-only.</p>
            </details>
          </div>
        </section>
      </main>

      <footer className="site-footer">
        <div className="site-footer-brand">
          <img src={installerIcon} alt="" width="40" height="40" />
          <strong>Aseprite Installer</strong>
        </div>
        <div className="site-footer-links">
          <a href={BUY_URL}>Buy official Aseprite <ExternalArrow /></a>
          <a href={EULA_URL}>Aseprite EULA <ExternalArrow /></a>
          <a href={`${GITHUB_URL}/blob/main/LICENSE`}>MIT License <ExternalArrow /></a>
        </div>
        <p>© 2026 fmhun. This independent project is not affiliated with, endorsed by, or supported by Igara Studio or GitHub.</p>
      </footer>
    </div>
  );
}

export default App;
