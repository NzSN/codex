# Workflow Strategy

## NzSN fork releases

`rust-release.yml` builds only `x86_64-unknown-linux-musl` on the standard
`ubuntu-24.04` GitHub runner. Push a `rust-v<version>` tag pointing to a commit
whose `codex-rs/Cargo.toml` workspace version matches the upstream version in
the tag. Stable fork tags may append a fourth numeric component for the NzSN
revision: `rust-v0.165.0.1` means NzSN revision 1 of upstream `0.165.0` and
requires Cargo workspace version `0.165.0`. NzSN revisions sort numerically
within an upstream release; an omitted revision is equivalent to revision 0.
Upstream stable and alpha/beta tags remain supported and require their exact
Cargo workspace version. For a retry, use the workflow's manual trigger and
provide the existing tag. Both triggers check out the tagged commit, so the
tagged commit must include the workflow and validation support for its format.

The release uploads only to `NzSN/codex` GitHub Releases. It does not invoke
npm, PyPI, R2, WinGet, website deployment, or signing services. The downstream
Python SDK release workflow is guarded to run only in `openai/codex`.

Assets include CLI and app-server packages with their code-mode host, ripgrep,
and sandbox resources, plus the responses API proxy, debug symbols, source
commit, configuration schema, and checksums. Extract the whole package and run
`bin/codex`; retain the adjacent resource directories. The binaries are unsigned
and do not include optional native voice runtime bundles.

The workflows in this directory are split so that pull requests get fast, review-friendly signal while `main` still gets the full cross-platform verification pass.

## Pull Requests

- Required checks run against GitHub's synthetic merge commit, not the pull
  request head alone. This includes changes already on `main` and catches
  conflicts before they reach the branch.
- `bazel.yml` is the main pre-merge verification path for Rust code.
  It runs Bazel `test` and Bazel `clippy` on the supported Bazel targets,
  including the generated Rust test binaries needed to lint inline `#[cfg(test)]`
  code.
- `rust-ci.yml` keeps the Cargo-native PR checks intentionally small:
  - `cargo fmt --check`
  - `cargo shear`
  - `argument-comment-lint` on Linux, macOS, and Windows
  - `tools/argument-comment-lint` package tests when the lint or its workflow wiring changes

## Post-Merge On `main`

- `bazel.yml` also runs on pushes to `main`.
  This re-verifies the merged Bazel path and helps keep the BuildBuddy caches warm.
- `rust-ci-full.yml` is the full Cargo-native verification workflow.
  It keeps the heavier checks off the PR path while still validating them after merge:
  - the full Cargo `clippy` matrix
  - the full Cargo `nextest` matrix via per-platform archive-backed shards
  - Windows ARM64 nextest archives cross-compiled on Windows x64, then replayed on native Windows ARM64 shards
  - release-profile Cargo builds
  - cross-platform `argument-comment-lint`
  - Linux remote-env tests

## Rule Of Thumb

- If a build/test/clippy check can be expressed in Bazel, prefer putting the PR-time version in `bazel.yml`.
- Keep `rust-ci.yml` fast enough that it usually does not dominate PR latency.
- Reserve `rust-ci-full.yml` for heavyweight Cargo-native coverage that Bazel does not replace yet.
