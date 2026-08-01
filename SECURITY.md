# Security policy

Please report security issues privately through GitHub's security advisory feature rather than opening a public issue.

## Trust boundaries

Aseprite Installer downloads Aseprite source archives only from the official `aseprite/aseprite` GitHub releases endpoint and requires the SHA-256 digest published in verified release metadata. Linux and Windows additionally download one exact official `aseprite/skia` release asset whose URL, size, and SHA-256 digest are pinned in the application. Every archive is verified before a traversal-safe extraction that rejects links, special files, collisions, and oversized content.

Builds use directly invoked CMake and Ninja executables with fixed argument lists and an isolated platform toolchain environment. Linux and Windows do not invoke upstream shell or batch scripts; CMake still evaluates the project files contained in the verified Aseprite source archive. The macOS implementation uses the verified upstream build workflow within the same constrained trust boundary.

Linux and Windows mutations use a durable transaction journal whose registry fingerprint chooses rollback or roll-forward after interruption. Recursive cleanup is authorized only after a verified artifact has been atomically renamed into a transaction-specific quarantine carrying an independent nonce proof. Desktop launchers are journaled as bounded, digested snapshots. If those proofs or a locked file prevent recovery, the application opens in read-only recovery mode and does not launch or mutate Aseprite until recovery succeeds.

The application does not request GitHub credentials, collect telemetry, or expose a general-purpose shell command to its webview.

## Supported versions

Security fixes are provided for the latest released version of Aseprite Installer.
