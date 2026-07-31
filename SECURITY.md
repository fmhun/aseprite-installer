# Security policy

Please report security issues privately through GitHub's security advisory feature rather than opening a public issue.

## Trust boundaries

Aseprite Installer downloads source assets only from the official `aseprite/aseprite` GitHub releases endpoint. It requires a GitHub-provided SHA-256 digest, validates archive paths, and executes only the `build.sh` contained in that verified official archive.

The application does not request GitHub credentials, collect telemetry, or expose a general-purpose shell command to its webview.

## Supported versions

Security fixes are provided for the latest released version of Aseprite Installer.
