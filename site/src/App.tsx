import { useRef, type ReactNode } from "react";
import installerIcon from "../../assets/icons/aseprite-installer.svg";
import localBuildIcon from "../../assets/icons/aseprite-local.svg";
import type { DemoPhase } from "./demo";
import { useDemoPlayback } from "./useDemoPlayback";

const GITHUB_URL = "https://github.com/fmhun/aseprite-installer";
const DOWNLOAD_URL = `${GITHUB_URL}/releases/latest/download/Aseprite-Installer-macOS-Universal.dmg`;
const BUY_URL = "https://www.aseprite.org/buy/";
const EULA_URL = "https://github.com/aseprite/aseprite/blob/main/EULA.txt";

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
        <h3>No local copy found</h3>
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
        <h3>Choose a release</h3>
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
  ["Xcode + macOS SDK", "16.2"],
  ["CMake", "3.31"],
  ["Ninja", "1.12"],
  ["Free space", "18 GB"],
] as const;

function PreflightScreen() {
  return (
    <>
      <DemoStepper current={1} />
      <div className="demo-panel demo-flow-panel">
        <div className="demo-kicker">STEP 2 OF 3</div>
        <h3>Ready to build</h3>
        <p>Your tools stay on your Mac.</p>
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

function EulaScreen() {
  return (
    <>
      <DemoStepper current={1} />
      <div className="demo-panel demo-flow-panel demo-panel--dimmed">
        <div className="demo-kicker">STEP 2 OF 3</div>
        <h3>Ready to build</h3>
        <p>Your tools stay on your Mac.</p>
        <ul className="demo-tool-list">
          {tools.slice(0, 3).map(([tool, version]) => (
            <li key={tool}><span className="demo-check">✓</span><strong>{tool}</strong><small>{version}</small></li>
          ))}
        </ul>
      </div>
      <div className="demo-modal-shade" />
      <div className="demo-modal">
        <div className="demo-modal-titlebar">PERSONAL BUILD / ASEPRITE <span>×</span></div>
        <div className="demo-document">≡</div>
        <h3>One legal check</h3>
        <p>The source build is for your own personal use under Aseprite’s EULA.</p>
        <div className="demo-consent"><span>✓</span> I understand and agree</div>
        <PixelButton>Continue →</PixelButton>
      </div>
    </>
  );
}

const stageCopy = {
  download: ["Downloading official source", "github.com/aseprite/aseprite"],
  verify: ["Verifying source archive", "SHA-256 digest matched"],
  compile: ["Compiling your local app", "build.sh --auto --norun"],
  sign: ["Signing local bundle", "ad-hoc signature applied"],
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
        <h3>{title}</h3>
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
        <img src={localBuildIcon} alt="" />
        <div className="demo-status-mark demo-status-mark--success">✓</div>
        <div>
          <div className="demo-kicker">BUILD COMPLETE</div>
          <h3>Your local app is ready</h3>
          <p>Aseprite · personal build</p>
          <code>~/Applications/Aseprite.app</code>
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
  if (phase === "eula") return <EulaScreen />;
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
            <div className="demo-traffic"><i /><i /><i /></div>
            <div className="demo-brand">
              <img src={installerIcon} alt="" />
              <span>Aseprite Installer</span>
            </div>
            <span className="demo-app-version">v0.1</span>
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
        The installer selects an official Aseprite release, checks local build tools, asks you to accept the upstream EULA, verifies and compiles the source, then installs your personal app in Applications.
      </figcaption>
    </figure>
  );
}

function App() {
  return (
    <div className="site-page">
      <a className="site-skip-link" href="#main">Skip to content</a>
      <header className="site-header">
        <a className="site-logo" href="#top" aria-label="Aseprite Installer home">
          <img src={installerIcon} alt="" />
          <span>Aseprite Installer</span>
          <small>OPEN SOURCE</small>
        </a>
        <nav className="site-nav" aria-label="Main navigation">
          <a href="#how-it-works">How it works</a>
          <a href="#install">Install</a>
          <a href="#open-source">Open source</a>
          <a href={GITHUB_URL}>GitHub <ExternalArrow /></a>
        </nav>
        <a className="site-button site-button--small" href={DOWNLOAD_URL}>Download</a>
      </header>

      <main id="main">
        <section className="site-hero" id="top">
          <div className="site-hero-copy">
            <p className="site-eyebrow"><span /> MADE FOR MACOS</p>
            <h1>Build Aseprite<br />from source. <em>Locally.</em></h1>
            <p className="site-lead">
              Pick an official release and turn it into your own macOS app. The installer verifies the source, compiles it on your machine, and replaces nothing until the build is ready.
            </p>
            <div className="site-hero-actions">
              <a className="site-button" href={DOWNLOAD_URL}>Download for macOS <span aria-hidden="true">↓</span></a>
              <a className="site-text-link" href={GITHUB_URL}>View source <ExternalArrow /></a>
            </div>
            <p className="site-compatibility">macOS 15.2+ <i /> Universal DMG <i /> Apple Silicon + Intel</p>
          </div>

          <div className="site-demo-wrap">
            <div className="site-demo-label"><span>LIVE WALKTHROUGH</span><small>A personal build, start to finish</small></div>
            <ProductDemo />
          </div>

          <p className="site-unofficial">
            Unofficial and unaffiliated. This tool does not distribute Aseprite binaries or replace the official paid edition.
          </p>
        </section>

        <section className="site-trust" aria-label="Project principles">
          <span><i>01</i> Verified source</span>
          <span><i>02</i> Built locally</span>
          <span><i>03</i> Safe replacement</span>
          <span><i>04</i> No telemetry</span>
        </section>

        <section className="site-section site-how" id="how-it-works">
          <div className="site-section-heading">
            <p className="site-eyebrow"><span /> HOW IT WORKS</p>
            <h2>Three steps.<br />One local app.</h2>
          </div>
          <ol className="site-process">
            <li>
              <div className="site-process-number">01</div>
              <div><h3>Pick a release</h3><p>Choose a stable Aseprite source archive from the official GitHub releases.</p></div>
            </li>
            <li>
              <div className="site-process-number">02</div>
              <div><h3>Check your Mac</h3><p>Confirm Xcode, CMake, Ninja, disk space, and the personal-use EULA.</p></div>
            </li>
            <li>
              <div className="site-process-number">03</div>
              <div><h3>Build safely</h3><p>Verify, compile, sign, and stage the app before it reaches <code>~/Applications</code>.</p></div>
            </li>
          </ol>
        </section>

        <section className="site-section site-install" id="install">
          <div className="site-install-copy">
            <p className="site-eyebrow"><span /> INSTALL</p>
            <h2>Drop in.<br />Build local.</h2>
            <ol className="site-install-steps">
              <li><span>1</span><div><strong>Download</strong><small>Get the universal DMG from the latest GitHub release.</small></div></li>
              <li><span>2</span><div><strong>Open the DMG</strong><small>Move the installer to Applications, or run it from the disk image.</small></div></li>
              <li><span>3</span><div><strong>First launch</strong><small>If macOS blocks it, Control-click the app and choose <b>Open</b>.</small></div></li>
            </ol>
            <a className="site-button" href={DOWNLOAD_URL}>Download universal DMG <span aria-hidden="true">↓</span></a>
          </div>

          <div className="site-dmg" aria-label="Illustration of the installer disk image">
            <div className="site-dmg-titlebar">
              <span><i /><i /><i /></span>
              <strong>Aseprite Installer</strong>
              <small>▦</small>
            </div>
            <div className="site-dmg-canvas">
              <div className="site-dmg-app">
                <img src={installerIcon} alt="" />
                <span>Aseprite Installer</span>
              </div>
              <div className="site-dmg-arrow"><i /><i /><i /></div>
              <div className="site-folder"><span>Applications</span></div>
            </div>
            <div className="site-dmg-status">2 items · read-only disk image</div>
          </div>

          <div className="site-requirements">
            <strong>BUILD REQUIREMENTS</strong>
            <span>macOS 15.2+</span><span>Xcode + SDK</span><span>CMake</span><span>Ninja</span><span>~6 GB free</span>
          </div>
          <div className="site-platforms" aria-label="Platform availability">
            <span><i className="site-dot site-dot--available" /> macOS <small>available</small></span>
            <span><i /> Windows <small>planned</small></span>
            <span><i /> Linux <small>planned</small></span>
          </div>
        </section>

        <section className="site-section site-source" id="open-source">
          <div>
            <p className="site-eyebrow"><span /> OPEN SOURCE</p>
            <h2>MIT licensed.<br /><em>Small enough to audit.</em></h2>
            <p>A transparent Tauri, React, and Rust utility. No account, token, analytics, or hidden build service.</p>
          </div>
          <nav className="site-source-links" aria-label="Open source project links">
            <a href={GITHUB_URL}><span>Browse the code</span><small>GitHub repository</small><ExternalArrow /></a>
            <a href={`${GITHUB_URL}/releases`}><span>Releases</span><small>DMGs and changelog</small><ExternalArrow /></a>
            <a href={`${GITHUB_URL}/issues`}><span>Issues</span><small>Report a bug</small><ExternalArrow /></a>
            <a href={`${GITHUB_URL}/security/policy`}><span>Security</span><small>Private reporting</small><ExternalArrow /></a>
          </nav>
        </section>

        <section className="site-section site-faq">
          <div className="site-section-heading">
            <p className="site-eyebrow"><span /> FAQ</p>
            <h2>The essentials.</h2>
          </div>
          <div className="site-faq-list">
            <details>
              <summary>Does the installer distribute Aseprite?<span>+</span></summary>
              <p>No. It downloads an official source archive only after you choose a release, verifies its GitHub-provided SHA-256 digest, and compiles your personal copy locally.</p>
            </details>
            <details>
              <summary>Why might macOS show a warning?<span>+</span></summary>
              <p>The first release is ad-hoc signed, not notarized with an Apple Developer ID. Control-click the installer, choose Open, then confirm once.</p>
            </details>
            <details>
              <summary>What happens to an existing copy?<span>+</span></summary>
              <p>The new app is staged and checked before replacement. Managed builds can keep a backup; Steam and package-manager copies remain read-only.</p>
            </details>
          </div>
        </section>
      </main>

      <footer className="site-footer">
        <div className="site-footer-brand">
          <img src={installerIcon} alt="" />
          <div><strong>Aseprite Installer</strong><small>Build it where it belongs: on your Mac.</small></div>
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
