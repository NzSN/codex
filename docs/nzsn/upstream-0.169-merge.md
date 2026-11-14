# rust-v0.169.0 preservation merge

The merge starts at NzSN main `ed039e4a24a706ba19dbeaa6ff194e1da7cb3db7`
and integrates upstream release commit
`d32063f96c2dff1dc96af839b3952abb6acfeda4` (`rust-v0.169.0`).
The annotated tag object is `a41fe5b443652cbdbee57717fbb9fd78d8d8b32a`.

## Preserved behavior

- Roles can select their own provider and catalog. Managed restrictions still apply,
  and child reload retains the child's owned context.
- Plaintext collaboration retains attributed input, payload validation, bounded
  compatibility messages, namespace selection, and queue-only delivery to evicted agents.
- Custom-provider output pairing and orphaned stream-delta handling remain in place.
- Thread contexts retain their provider, catalog, credential owner, and originator.
  Cold resume, concurrent local clients, and automatic review retain that ownership.
- TUI startup retains explicit invocation/profile choices and host account identity.
  Session switches restore running tools and respect completions received during hydration.
- Releases remain GitHub-only Linux x86_64 builds, support NzSN revision suffixes,
  install the source-selected Rust toolchain, and archive symbols before stripping.

## Reconciliation with upstream

Upstream's runtime-auth catalog factory now coexists with resolved thread contexts.
Rebinding a context changes its credential/runtime owner while retaining its catalog
snapshot, originator partition, and isolated cache. A new provider selector cannot
redirect a context-scoped model-list request. Saved-provider selection is a
fallback on resume; explicit replacements retain their selection while both
saved and requested routes are checked against managed requirements.

Upstream's request-only TUI session views retain the negotiated mixed-provider
capability and explicit launch choices. Native agent follow-up submission enforces
the recipient's payload mode, provider representation, and plaintext size bound.

The retained reviewer/catalog checks use upstream's `CatalogModel` provenance.
Internal rollout metadata follows upstream's JSON-schema contract; its plaintext
payload field remains optional and defaults to encrypted for legacy histories.
Fixtures include the new optional account, token-count, namespace, and native-call
fields. Stable/experimental protocol exports and Python bindings are regenerated
from their Rust owners.

Windows ACL resolution retains the caller's directory-only `WRITE_DAC` restriction
alongside upstream's protected-DACL SYSTEM maintenance access. Native Windows
execution was not exercised.

Cargo refreshed stale workspace package versions to `0.169.0`. Dependency review
found no additional resolution drift beyond those versions and the temporary V8
source substitution described below.

## Validation

The following scopes passed on 2026-11-14: **1,143 distinct selected Rust tests**,
29 Python SDK contract/signature tests, and 59 release/package helper tests.

| Rust validation scope | Passed |
| --- | ---: |
| Complete protocol suite | 377 |
| Complete app-server protocol suite | 327 |
| Complete model-provider suite | 146 |
| Complete models-manager suite | 60 |
| Selected core preservation and upstream ownership regressions | 111 |
| Selected app-server context/provider/resume units | 28 |
| Selected TUI startup/provider/activity/status regressions | 39 |
| Native core plaintext/stream/catalog/Guardian integrations | 13 |
| Native app-server mixed-provider/review/policy/model-list integrations | 40 |
| Exact CLI mixed-provider and profile-model launch fixtures | 2 |

The app-server protocol suite has one ignored generation helper, invoked separately
for stable and experimental generation. The initial TUI update-message failure was
resolved through its test owner; the only accepted change was `0.166.0` to `0.169.0`.
Two historical startup test names were replaced upstream; the complete current
startup-defaults module is included in the selected TUI run.

The native CLI, app-server, matching code-mode host, and config-schema generator
built successfully. The CLI reports `codex-cli 0.169.0`; owner-driven config-schema
generation succeeds. Python validation uses the frozen Python 3.12 environment;
release helpers pass 32 tests and package helpers pass 27.

Local-listener tests run outside the listener-restricted sandbox with its inherited
`CODEX_SANDBOX_NETWORK_DISABLED` skip flag removed. The initial 0.02-second native
run returned early under that flag and receives no validation credit. Actual
execution exposed a retained Guardian plaintext fixture calling the encrypted
namespace; its wire call now uses `collaboration_plaintext`. Both encrypted and
plaintext denial/reporting controls pass. The explicit-resume replacement controls
also pass for typed input, config input, and saved-route fallback.

Detailed commands, compiled inventories, pass/failure receipts, and dependency
identity checks are retained locally under `.cache/upstream-0.169-merge/`.
Publication manifests retain the upstream Git V8 dependency. Final formatting,
Bazel-lock, staging, and ancestry checks are recorded with those receipts.
`just fmt-check`, `just bazel-lock-update`, and `just bazel-lock-check` pass.
Bazel's existing dependency-version warnings remain visible in the logs.

## Local build identity and limits

Local validation uses Rust 1.95.0 and native GNU Linux development builds. Cargo's
V8 source-submodule fetch was replaced for validation by an archive of the exact
pinned source revision `90a65f887a0c0f048f3784afbcb320b373c8a2e0`; all 163 Git
blobs were verified. The native archive and bindings are checked against the
repository's trusted V8 release checksum manifest. The published Cargo manifest
and lockfile retain upstream's Git dependency. Source-blob identity was rechecked
after the native build.

Early compile failures identified API/fixture drift and were repaired before the
completed suites. A native build exhausted the filesystem; completed incremental
compiler caches were removed and the build restarted. Retained release packages
were not removed. Initial Bazel lock generation rejected the temporary V8 path;
final lock generation uses the restored publication manifests.

Whole-merge whitespace warnings are confined to ten snapshot/patch files whose
bytes match the upstream tag. The fork delta against the tag passes whitespace
checking. The pre-existing `.projectile-cache.eld` is excluded.

This is selected local source validation. It does not establish full-workspace,
live-provider, native-Windows, native V8 source-build, musl release-package, or
installed-daemon acceptance. No installation, release, or push is part of this task.
