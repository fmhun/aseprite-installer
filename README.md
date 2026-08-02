# Aseprite Installer

An unofficial macOS, Windows, and Linux utility that compiles and manages a personal Aseprite installation from official source releases.

[Website](https://fmhun.github.io/aseprite-installer/) · [Download for macOS, Windows, or Linux](https://github.com/fmhun/aseprite-installer/releases/latest)

> [!IMPORTANT]
> Aseprite is not distributed under a conventional open-source license. Its EULA allows you to compile and modify the source code for your own personal purpose, but it does not allow third-party redistribution of compiled Aseprite binaries. This project never bundles or publishes Aseprite, its source archive, Skia, or Aseprite branding.

The official paid edition is the recommended way to get Aseprite: it funds Igara Studio and includes signed packages, automatic updates, a Steam key, and priority support. The installer presents that option before offering a personal source build and never describes the locally compiled copy as a free edition.

## Supported platforms

| Platform | Supported systems | Installer packages |
| --- | --- | --- |
| macOS | macOS 15.2+, Apple Silicon and Intel | Architecture-specific app archive and DMG |
| Windows | Windows 11 x64 | NSIS current-user installer and MSI for managed deployment |
| Linux | x86_64, Ubuntu 22.04/Debian 12 or a modern compatible distribution | AppImage, deb, and rpm |

Windows ARM64, Linux ARM64, Windows 10, and Ubuntu 20.04 are not supported by the desktop installer. The AppImage is the least invasive Linux package. Do not install the NSIS and MSI packages side by side: use NSIS for a personal installation or MSI for centrally managed deployment.

## What it does

- Detects managed, manual, Steam, and package-manager Aseprite installations without taking ownership of read-only copies.
- Lists verified Aseprite 1.3 source releases from GitHub, with stable releases shown by default.
- Checks the native compiler, SDK, CMake, Ninja, architecture, filesystem behavior, and free disk space before making changes.
- Downloads official Aseprite source and Skia release assets into a local cache and verifies their pinned sizes and SHA-256 digests.
- Configures and compiles Aseprite locally with CMake and Ninja; the installer never uploads the result.
- Stages and validates the complete result before transactionally replacing the active managed copy.
- Integrates a managed copy with Applications on macOS, the current user's Start menu on Windows, or the current user's desktop menu on Linux.
- Supports updates, downgrades, reinstalls, cancellation, backup restore, and recoverable uninstall while preserving artwork and preferences.

Aseprite Installer itself has no built-in updater. Install a newer package from this repository to update the installer.

## Privacy and safety

- No telemetry, analytics, account, login, or GitHub token is used by the application.
- The frontend cannot execute arbitrary commands or choose unchecked filesystem targets.
- Downloads are verified before extraction, and archives are rejected for traversal, links, special files, unsafe collisions, or unexpected size.
- Build commands use fixed executable paths and arguments rather than interpolated shell commands.
- A platform-specific process group or Windows process-tree termination stops the compiler tree on cancellation so background builds are not left running.
- The new application is staged and validated before the active copy is replaced. A durable transaction journal repairs an interrupted install, restore, desktop-integration change, or uninstall on the next launch. Verified trees are atomically isolated into transaction-bound quarantines before recursive cleanup, eliminating path-swap deletion races.
- If automatic recovery is temporarily blocked by a locked file or changed safety proof, the installer still opens in read-only recovery mode. It displays the journal and diagnostic, blocks launch/mutation commands, and offers a bounded retry instead of exiting or guessing.
- Aseprite Installer never elevates itself or silently changes system tools. Linux and Windows remediation commands are instructions only; on macOS, the user can explicitly authorize the existing Homebrew action for CMake and Ninja when a safe Homebrew installation is detected.
- Existing artwork, palettes, scripts, extensions, and preferences are never removed.

## Build requirements

All platforms need roughly 6 GB of temporary free space plus room for the managed installation.

### macOS

- macOS 15.2 or newer on Apple Silicon or Intel
- Xcode with its macOS SDK and command-line tools
- CMake 3.20 or newer
- Ninja 1.10 or newer

If a safe Homebrew installation is already present, the user may explicitly authorize the installer’s CMake/Ninja action; the tools can also be installed manually. The installer does not install Homebrew or Xcode.

### Windows

- Windows 11 x64
- Visual Studio 2022 with **Desktop development with C++**, the x64 MSVC tools, and Windows SDK 10.0.26100
- CMake 3.20 or newer and Ninja 1.10 or newer, either standalone or supplied by Visual Studio

MinGW, WSL, emulated ARM, and cross-compilation are not supported. The application only reads the Visual Studio environment it needs and never launches Visual Studio Installer itself.

### Linux

- A glibc-based x86_64 desktop compatible with the Ubuntu 22.04/Debian 12 Tauri runtime baseline
- Clang and Clang++ 12 or newer, CMake 3.20 or newer, and Ninja 1.10 or newer
- Development libraries for X11, Xcursor, XInput, XRandR, OpenGL, and fontconfig
- WebKitGTK 4.1 and the runtime dependencies required by the chosen AppImage, deb, or rpm package

The application shows a distribution-specific apt, dnf, pacman, or zypper command when it recognizes the system. It never executes `sudo` or `pkexec`.

## Development

The project uses Tauri 2, React 19, TypeScript, Vite, and Rust.

```bash
npm install
npm run tauri dev
```

### Landing-page platform simulation

The deployed landing page exposes a namespaced, allow-listed simulator for browser QA. In Chrome DevTools, run:

```js
AsepriteInstaller.platform.help()
AsepriteInstaller.platform.simulate("macos-arm64")
AsepriteInstaller.platform.simulate("macos-x64")
AsepriteInstaller.platform.simulate("windows")
AsepriteInstaller.platform.simulate("linux")
AsepriteInstaller.platform.simulate("mobile")
AsepriteInstaller.platform.state()
AsepriteInstaller.platform.list()
AsepriteInstaller.platform.reset()
```

Simulations update the page immediately without modifying `navigator` or initiating a download. They are memory-only by default. To keep one for the current tab across reloads, pass `{ persist: "session" }`. `reset()` clears the override and its session value; if browser storage becomes inaccessible, it fails atomically instead of reporting a false reset.

Quality checks:

```bash
npm run lint
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Tauri loads common settings from `src-tauri/tauri.conf.json` and automatically merges the matching `tauri.macos.conf.json`, `tauri.linux.conf.json`, or `tauri.windows.conf.json` override. Native bundles must be built and inspected on their matching operating system.

The two editable SVG files in `assets/icons/` are the source of truth and must be preserved. Generated PNG, ICO, and ICNS files should always be rebuilt from those masters:

```bash
npm run tauri icon assets/icons/aseprite-installer.svg
```

The installer icon uses the open-package variant everywhere. Aseprite copies compiled and managed locally receive the loader-only native icon where the platform supports it. Both marks use the same four-color, dark-framed loader from the interface.

The stepped outer silhouette intentionally references Aseprite's familiar application shape, but the checkerboard, package, and loader artwork are project-specific. No upstream image, GitHub logo, Octocat, or wordmark is bundled. The self-hosted [Pixelify Sans](https://fonts.google.com/specimen/Pixelify+Sans) interface font is licensed under the SIL Open Font License 1.1; its license is included beside the font files.

## Releases and verification

Tags matching the application version build four native targets in isolated GitHub Actions jobs:

- macOS arm64 and x64: `.app.zip` and `.dmg`
- Linux x86_64 on Ubuntu 22.04: `.AppImage`, `.deb`, and `.rpm`
- Windows x64: current-user NSIS `.exe` and managed-deployment `.msi`

The release workflow inspects every package, enforces an exact artifact allow-list, creates `SHA256SUMS`, generates GitHub build-provenance attestations, and publishes only after every platform succeeds. Linux qualification includes a normal FUSE-backed AppImage launch, deb installation/lifecycle tests on Ubuntu 22.04 and Debian 12, and an rpm lifecycle test on Fedora. Windows qualification verifies the NSIS execution level and per-user registration, then installs, opens, fully unregisters, and removes both NSIS and MSI packages while checking their x64 payloads and shortcuts. GitHub's Windows Server 2025 runner is used as the packaging proxy; the application itself separately enforces Windows 11 x64 during preflight. CI and release jobs never download, compile, cache, or publish Aseprite or Skia.

Release tags must use a stable `vMAJOR.MINOR.PATCH` version, resolve to the workflow commit, and belong to `main`; the workflow rechecks the remote tag immediately before publication. Node.js 24.15.0, Rust 1.97.1, container images, and third-party actions are pinned for repeatability. Repository administrators should additionally protect `v*` tags with a GitHub ruleset so only the intended release process can create or update them.

Release packages are intentionally not backed by commercial signing certificates:

- macOS apps are ad-hoc signed but not notarized, so Gatekeeper may require Control-clicking the app and choosing **Open** the first time.
- Windows packages are not Authenticode-signed, so SmartScreen may warn. Verify the checksum and provenance before choosing **More info → Run anyway**.
- Linux packages are unsigned. The AppImage may need `chmod +x` after downloading.

After downloading an asset and `SHA256SUMS`, verify that individual asset by filtering its exact manifest entry. For example on Linux:

```bash
asset=Aseprite-Installer-Linux-x86_64.AppImage
grep -F "  $asset" SHA256SUMS | sha256sum --check -
gh attestation verify Aseprite-Installer-Linux-x86_64.AppImage --repo fmhun/aseprite-installer
```

On macOS, set `asset` to the downloaded DMG or app archive and use `grep -F "  $asset" SHA256SUMS | shasum -a 256 -c -`. Download all nine release assets only if you want to run `sha256sum --check SHA256SUMS` against the complete manifest.

## Official references

- [Aseprite installation instructions](https://github.com/aseprite/aseprite/blob/main/INSTALL.md)
- [Aseprite EULA](https://github.com/aseprite/aseprite/blob/main/EULA.txt)
- [Aseprite releases](https://github.com/aseprite/aseprite/releases)
- [Aseprite theme documentation](https://www.aseprite.org/docs/extensions/themes/)

This project is not affiliated with, endorsed by, or supported by Igara Studio or GitHub.
