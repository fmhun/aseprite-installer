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
  ["Xcode + macOS SDK", "16.2"],
  ["CMake", "3.31"],
  ["Ninja", "1.12"],
  ["Free space", "~6 GB"],
] as const;

function PreflightScreen() {
  return (
    <>
      <DemoStepper current={1} />
      <div className="demo-panel demo-flow-panel">
        <div className="demo-kicker">STEP 2 OF 3</div>
        <p className="demo-panel-title">Ready to build</p>
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
              <img src={installerIcon} alt="" width="26" height="26" />
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
        The installer selects an official Aseprite release, checks local build tools, verifies and compiles the source, then installs your personal app in Applications.
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
        <a className="site-button site-button--small" href={DOWNLOAD_URL}>Download</a>
      </header>

      <main id="main" tabIndex={-1}>
        <section className="site-hero" id="top">
          <div className="site-hero-copy">
            <h1>Install <em>Aseprite</em><br />from source.</h1>
            <p className="site-eyebrow site-hero-tag"><span /> AND FOR FREE</p>
            <p className="site-lead">
              Aseprite Installer is a free, MIT-licensed macOS utility that checks the required build tools, verifies an official source release, and compiles your personal Aseprite copy locally.
            </p>
            <div className="site-hero-actions">
              <a className="site-button" href={DOWNLOAD_URL}>Download for macOS <span aria-hidden="true">↓</span></a>
              <a className="site-text-link" href={GITHUB_URL}>View source on GitHub <ExternalArrow /></a>
            </div>
            <p className="site-compatibility">macOS 15.2+ <i /> Universal DMG <i /> Apple Silicon + Intel</p>
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
            <h2>Drop in.<br />Build local.</h2>
            <ol className="site-install-steps">
              <li><span>1</span><div><strong>Download</strong><small>Get the universal DMG from the latest GitHub release.</small></div></li>
              <li><span>2</span><div><strong>Open the DMG</strong><small>Move the installer to Applications, or run it from the disk image.</small></div></li>
              <li><span>3</span><div><strong>First launch</strong><small>If macOS blocks it, Control-click the app and choose <b>Open</b>.</small></div></li>
            </ol>
            <a className="site-button" href={DOWNLOAD_URL}>Download universal DMG <span aria-hidden="true">↓</span></a>
          </div>

          <div className="site-dmg" role="img" aria-label="Illustration of the installer disk image">
            <div className="site-dmg-titlebar">
              <span><i /><i /><i /></span>
              <strong>Aseprite Installer</strong>
              <small>▦</small>
            </div>
            <div className="site-dmg-canvas">
              <div className="site-dmg-app">
                <img src={installerIcon} alt="" width="88" height="88" />
                <span>Aseprite Installer</span>
              </div>
              <div className="site-dmg-arrow"><i /><i /><i /></div>
              <div className="site-folder"><span>Applications</span></div>
            </div>
            <div className="site-dmg-status">2 items · read-only disk image</div>
          </div>

          <div className="site-requirements">
            <span>macOS 15.2+</span><span>Xcode + SDK</span><span>CMake</span><span>Ninja</span><span>~6 GB free</span>
          </div>
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
              <div><h3>Get ready to build</h3><p>Let the installer set up CMake and Ninja through Homebrew, or follow the built-in guides to resolve each requirement manually.</p></div>
            </li>
            <li>
              <div className="site-process-number">03</div>
              <div><h3>Build safely</h3><p>Verify, compile, sign, and stage the app before it reaches <code>~/Applications</code>.</p></div>
            </li>
          </ol>
        </section>

        <section className="site-section site-source" id="open-source">
          <div>
            <h2>MIT licensed.<br /><em>OPEN SOURCE</em></h2>
            <p>Aseprite Installer is a transparent Tauri, React, and Rust utility released under the MIT License. Aseprite remains subject to its own EULA. No account, token, analytics, or hidden build service.</p>
          </div>
          <nav className="site-source-links" aria-label="Open source project links">
            <a href={GITHUB_URL}><span>Browse the code</span><small>GitHub repository</small><ExternalArrow /></a>
            <a href={`${GITHUB_URL}/releases`}><span>Releases</span><small>DMGs and changelog</small><ExternalArrow /></a>
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
              <summary>Does the installer distribute Aseprite?<span aria-hidden="true">+</span></summary>
              <p>No. It downloads an official source archive only after you choose a release, verifies its GitHub-provided SHA-256 digest, and compiles your personal copy locally.</p>
            </details>
            <details>
              <summary>Why might macOS show a warning?<span aria-hidden="true">+</span></summary>
              <p>The current release is ad-hoc signed and not notarized by Apple; it does not use a Developer ID certificate. Control-click the installer, choose Open, then confirm once.</p>
            </details>
            <details>
              <summary>What happens to an existing copy?<span aria-hidden="true">+</span></summary>
              <p>The new app is staged and checked before replacement. Managed builds can keep a backup; Steam and package-manager copies remain read-only.</p>
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
