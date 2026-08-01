# Changelog

All notable changes to Aseprite Installer are documented here.

## Unreleased

- Added native Windows 11 x64 support with a current-user NSIS installer and an MSI for managed deployment.
- Added Linux x86_64 support with AppImage, deb, and rpm packages built on Ubuntu 22.04.
- Split the Tauri bundle configuration into shared, macOS, Linux, and Windows policies with updater artifacts explicitly disabled.
- Added native Rust checks, tests, bundle builds, and package inspections for macOS arm64/x64, Linux x64, and Windows x64, including Ubuntu 22.04, Debian 12, Fedora, NSIS, and MSI lifecycle smoke tests.
- Added a durable transaction journal with restart recovery for interrupted Linux and Windows install, restore, and uninstall operations.
- Added transaction-bound cleanup quarantines, recoverable desktop-integration snapshots, safe read-only startup recovery, and launch/mutation locking on Linux and Windows.
- Reworked releases into isolated builds followed by one allow-listed collection and publication gate with SHA-256 checksums, GitHub provenance attestations, stable-version enforcement, main-branch ancestry checks, and last-moment remote-tag verification.
- Pinned Rust, Linux smoke images, and GitHub Actions to immutable versions and added grouped Dependabot updates for actions, npm, and Cargo dependencies.
- Documented ad-hoc macOS signing and unsigned Linux/Windows distribution, including Gatekeeper and SmartScreen guidance.

## 0.1.0 — 2026-07-31

- Initial macOS release for Apple Silicon and Intel.
- Verified Aseprite 1.3 source-release selection.
- Detection of managed, manual, Steam, and package-manager installations.
- Guided prerequisites and optional Homebrew installation of CMake/Ninja.
- Cancellable local compilation with live logs.
- Atomic installation, adoption backup, rollback, and Trash-based uninstall.
- English interface for the initial macOS release.
- Context-aware installation flow with dedicated release, prerequisite, and build screens.
- Pixel-art installer icon plus a dedicated icon for locally managed Aseprite bundles.
- Self-hosted Pixelify Sans interface typography with improved small-text readability.
- Accessible typography scale with 14 px body text and 12 px minimum functional text.
- Official-purchase path and development-support reminders before, during, and after personal source compilation.
