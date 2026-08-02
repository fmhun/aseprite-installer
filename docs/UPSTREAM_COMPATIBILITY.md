# Aseprite upstream compatibility

`upstream/aseprite-compatibility.json` is the source of truth for the Aseprite
release and build procedure reviewed by this project. It separates three facts
that must not be conflated:

- `baseline_release` identifies the latest compatibility-reviewed release,
  including its GitHub release/asset IDs, immutable commit, official source-asset
  digest, and size. The release and asset are rechecked because GitHub reports
  this upstream release as mutable.
- `tracked_files.*.baseline_blob_sha` identifies the file content in that release.
- `tracked_files.*.observed_main_blob_sha` records the latest upstream `main`
  content that a maintainer has reviewed.
- `implementation_origin_commit` identifies the Aseprite Installer commit whose
  original build implementation was derived from that upstream procedure.

The application accepts Aseprite 1.3 releases only through
`compatibility.reviewed_through`. A newer release remains hidden until its build
contract has been reviewed. This preserves existing downgrade choices without
assuming that future 1.3 releases are compatible.

That boundary applies on every supported platform. Linux and Windows also keep
their narrower `portable_source_supported` gate for the separately pinned Skia
toolchain. The `tracked_files.build.sh` invocation is scoped to macOS, where the
installer runs the official script; portable installers reproduce the reviewed
steps with fixed CMake arguments and must be reviewed independently.

The manifest pins the exact identity of the latest reviewed baseline, not every
historical downgrade. Older releases continue to rely on the current metadata
and digest returned by the official GitHub repository. Expanding this into a
complete immutable allow-list would be a separate policy change.

## Automated watch

`.github/workflows/watch-aseprite-upstream.yml` runs weekly and can also be run
manually. Its checker compares the manifest with:

- `INSTALL.md` and `build.sh` on upstream `main`;
- the same files at the recorded baseline tag;
- the baseline tag commit and source-asset digest;
- newer Aseprite 1.3 releases, including prereleases, and the appearance of a
  newer release series such as 1.4 or 2.x.

Drift creates or refreshes one issue labelled `upstream-compatibility`. Duplicate
watcher issues are closed in favour of the oldest canonical issue. The
workflow intentionally never edits the manifest or enables a release: detecting
a change is not proof of compatibility.

Upstream reads are public and unauthenticated. The repository `GITHUB_TOKEN` is
reserved for the local compatibility issue and has only `contents: read` and
`issues: write` permissions.

GitHub can delay scheduled workflows and disables them after 60 days without
repository activity in a public repository. Maintainers should also watch the
upstream Aseprite Releases feed; manual workflow dispatch remains available.

## Review procedure

When the watcher reports drift:

1. Read the upstream diff for `INSTALL.md` and `build.sh`.
2. Check changes to required tools, platform/compiler/SDK versions, source-archive
   layout, Skia selection, and expected outputs. On macOS, review the `build.sh`
   arguments and the installer's case-safe app-bundle discovery. On Linux and
   Windows, review the pinned Skia assets and fixed CMake configuration.
3. Confirm the candidate is an official release asset and record its GitHub
   digest and resolved tag commit.
4. Update prerequisite checks, help text, release parsing, and installer logic as
   needed.
5. Run the frontend and Rust quality checks documented in `README.md`.
6. Before raising `reviewed_through`, complete local builds for the supported
   macOS arm64, macOS x64, Linux x64, and Windows x64 routes. Do not upload,
   cache, or redistribute the resulting Aseprite or Skia binaries.
7. Update `baseline_release`, `compatibility.reviewed_through`, the baseline file
   identities, and the observed `main` identities together in the same pull
   request as any compatibility fixes.

For a documentation-only change on upstream `main`, update only
`observed_main_blob_sha` and `last_reviewed_change_commit` after review. Do not
change the immutable baseline release fields unless a new release has actually
been validated.

Merging a manifest or watcher change activates the scheduled repository check.
Application-side compatibility changes reach users only after a new Aseprite
Installer release is built and published.

## Deliberate exclusions

The scheduled watcher performs metadata and content-identity checks only. It
does not execute upstream `main`, compile Aseprite in public CI, cache Aseprite or
Skia, or publish build artifacts. A future build-validation workflow must use a
pinned release and verified digest and should be introduced as a separate,
explicit policy decision.
