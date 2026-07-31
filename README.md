# Aseprite Installer

An unofficial macOS utility that compiles and manages a personal Aseprite installation from official source releases.

> [!IMPORTANT]
> Aseprite is not distributed under a conventional open-source license. Its EULA allows you to compile and modify the source code for your own personal purpose, but it does not allow third-party redistribution of compiled Aseprite binaries. This project never bundles or publishes Aseprite, its source archive, or its branding.

## What it does

- Detects managed, manual, Steam, and package-manager Aseprite installations.
- Lists verified Aseprite 1.3 source releases from GitHub.
- Shows stable releases by default and optionally includes betas/RCs.
- Checks Xcode, the macOS SDK, CMake, Ninja, architecture, and free disk space.
- Downloads the official `Aseprite-v…-Source.zip` asset and verifies GitHub's SHA-256 digest.
- Runs the release's official `build.sh --auto --norun` script locally.
- Applies an ad-hoc signature and installs to `~/Applications/Aseprite.app`.
- Supports updates, downgrades, reinstalls, cancellation, one-step backup restore, and recoverable uninstall through the Trash.
- Keeps Steam and package-manager copies read-only and delegates their updates to the original channel.

The first release supports macOS 15.2 or newer on Apple Silicon and Intel. The backend has a platform adapter boundary for future Windows and Linux support.

## Privacy and safety

- No telemetry, analytics, login, or GitHub token.
- The frontend cannot execute arbitrary shell commands.
- External links and installation paths are allow-listed in Rust.
- ZIP entries are checked against path traversal and symlinks.
- The new application is staged and validated before the active copy is replaced.
- Existing artwork and preferences are never removed.

## Requirements

- macOS 15.2+
- Xcode with a macOS SDK and command-line tools
- CMake
- Ninja
- About 6 GB of temporary free space

If Homebrew is already installed, Aseprite Installer can run the fixed command `brew install cmake ninja` after confirmation. It never installs Homebrew or Xcode automatically.

## Development

The project uses Tauri 2, React 19, TypeScript, Vite, and Rust.

```bash
npm install
npm run tauri dev
```

Quality checks:

```bash
npm run lint
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

The editable SVG sources and web PNG exports live in `assets/icons/`. Regenerate the native installer icon set from the vector master with:

```bash
npm run tauri icon assets/icons/aseprite-installer.svg
```

The installer icon uses the open-package variant. Aseprite bundles compiled and managed locally receive the loader-only variant before their ad-hoc signature is applied. Both marks use the exact four-color, dark-framed loader from the interface.

The stepped outer silhouette intentionally references Aseprite's familiar application shape, but the checkerboard, package, and loader artwork are project-specific. No upstream image file, GitHub logo, Octocat, or wordmark is bundled. This remains an unofficial, unaffiliated project.

The interface uses an original pixel-art design system inspired by the general visual language of pixel editors: integer-pixel borders, compact controls, checkerboard workspaces, and explicit raised/pressed states. It does not include or redistribute Aseprite's theme files, fonts, spritesheet, icons, or other visual assets.

The bundled UI typeface is [Pixelify Sans](https://fonts.google.com/specimen/Pixelify+Sans), licensed under the SIL Open Font License 1.1. Its license is included next to the font file in `src/assets/fonts/`.

## Release builds

Tags matching `v*` trigger GitHub Actions builds for Apple Silicon and Intel. The installer is ad-hoc signed until a Developer ID certificate is available, so macOS may require right-clicking the app and choosing **Open** the first time.

CI builds only Aseprite Installer. It never downloads, compiles, caches, or uploads Aseprite.

## Official references

- [Aseprite installation instructions](https://github.com/aseprite/aseprite/blob/main/INSTALL.md)
- [Aseprite EULA](https://github.com/aseprite/aseprite/blob/main/EULA.txt)
- [Aseprite releases](https://github.com/aseprite/aseprite/releases)
- [Aseprite theme documentation](https://www.aseprite.org/docs/extensions/themes/)

This project is not affiliated with, endorsed by, or supported by Igara Studio or GitHub.

---

## Français

**Aseprite Installer** est un utilitaire macOS communautaire et non officiel. Il télécharge une archive source officielle d'Aseprite, vérifie son empreinte SHA-256, la compile sur votre Mac puis gère l'application obtenue.

Aucun binaire Aseprite n'est distribué. Avant la compilation, l'application affiche l'EULA et exige la confirmation que la compilation est destinée à un usage personnel.

Fonctions principales :

- détection des copies manuelles, gérées, Steam et package manager ;
- choix d'une release Aseprite 1.3 stable ou d'une préversion ;
- diagnostic Xcode/SDK/CMake/Ninja et espace disque ;
- compilation annulable avec journal en direct ;
- installation sûre dans `~/Applications`, sauvegarde, restauration et Corbeille ;
- interface en anglais pour cette première version macOS.

Pour développer :

```bash
npm install
npm run tauri dev
```

Licence du présent installateur : MIT. Aseprite reste soumis à sa propre EULA.

L'interface possède son propre système visuel pixel-art. Aucun fichier de thème, police, spritesheet, icône ou autre asset visuel d'Aseprite n'est inclus ou redistribué.

La fonte d'interface embarquée est [Pixelify Sans](https://fonts.google.com/specimen/Pixelify+Sans), distribuée sous SIL Open Font License 1.1. Sa licence accompagne le fichier de fonte dans `src/assets/fonts/`.
