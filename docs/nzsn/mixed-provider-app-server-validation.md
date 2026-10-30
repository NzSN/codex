# Mixed Provider App Server implementation and validation

Status: implementation present; selected local qualification passed. Live-provider and native Windows qualification remain open.

Date: 2026-10-27. Starting checkout: `91876a35008ee2ed7efedb62c2519a0105f09482`.

Design: [Mixed Provider App Server](mixed-provider-app-server-design.md).

Follow-up (2026-10-28): [Provider-bound automatic approval review](provider-bound-auto-review.md)
documents the Kimi legacy-resume failure, the implemented same-provider/model reviewer
policy, explicit catalog recovery, and qualification of the follow-up fixes for runtime
catalog ownership, trusted-skill evidence, and stale default-model handling.

## Implementation scope

- Core provider/catalog contexts and scoped models managers.
- Versioned thread-settings metadata and immutable catalog objects addressed by their digest.
- Bounded, connection-owned pending handles with authentication owner-generation checks.
- Context resolution, scoped model listing/capabilities, and thread start/resume/fork attachment.
- A distinct local-socket connection origin for server-side catalog-path admission.
- CLI flag classification and TUI bootstrap, model picker, lifecycle, and reconnect integration.
- A durable reader-version marker and a required startup argument for managed daemon launches.

Catalog objects are committed before publishing thread-owned digest references through the existing rollout writer. The first implementation retains unreferenced objects rather than collecting them without a complete proof that no saved thread or fork refers to them. No automatic catalog-object deletion is authorized by this feature.

The managed-launcher compatibility check cannot retroactively change a released older executable. Manually invoking an unaware old server against the same store remains outside the supported downgrade path.

## Implementation refinements

- A context fixes provider and catalog; a validated top-level model choice may vary within it. This preserves startup model prompts without rereading the catalog or allowing a caller to replace the route through duplicate fields.
- The TUI defers user catalog I/O until server-side context resolution. Resume restores catalog objects before loading user configuration, including when the original default catalog file is missing. Managed catalog requirements remain checked.
- In-session forks inherit the actual source context; they do not reapply the connection's original provider flags. Reconnect uses thread identity rather than reusing connection-local handles.
- Authentication-owner changes invalidate pending handles; normal credential refresh under the same owner does not. Existing native verification restrictions also apply to the newly distinguished local-socket origin.
- Durable fingerprints describe public routing and credential references, excluding secret values and transport-only client-version headers. Ambiguous inline URI credentials and unsupported static header/query forms are rejected; host-owned environment header references are supported.

## Executed local qualification

All Rust test commands used `RUST_MIN_STACK=8388608`, matching the repository's test convention. The installed `+stable` toolchain reports Rust 1.99.1, matching the pinned compiler version. Counts below count distinct tests once, not reruns.

| Check | Result and boundary |
| --- | --- |
| CLI/server/TUI compilation | Passed `cargo +stable check -p codex-cli -p codex-app-server -p codex-tui --tests --offline`. |
| Core context unit tests | 7 passed: validation, bounded loading, metadata isolation, durable object integrity, tenant identity, credential rotation, and custom-provider picker filtering. |
| Pending-context registry | 2 passed: connection/owner binding, expiry, capacity, and disconnect cleanup. |
| TUI launch choices | 2 passed: exact Kimi selections/model-flag precedence and model/effort forwarding with a resolved context. |
| Daemon eligibility | 1 passed: admitted typed model selections retain exclusion of unsupported overrides. |
| Mixed-provider app-server integration | 5 passed: independent routes/instructions and deletion/restart, simultaneous local clients and handle ownership, conflicting route rejection, visible provider defaults, and authoritative native discovery/unknown-model rejection. Both original catalog files are deleted before the cold-resume assertion. |
| Exact CLI/daemon flow | 1 PTY test passed with the Kimi provider/catalog/model flags. It confirms a named Kimi session, shared-server status, and unchanged pre-existing daemon PID. The fixture has an isolated Codex home and does not call Kimi. |
| Existing collaboration delivery | 14 passed, including the OpenAI/DeepSeek delivery matrix and bounded delivery cases. Synthetic handler-level evidence only. |
| Existing role behavior | 24 passed. |
| Existing provider APIs and requirements | 7 passed, including provider capability reads and retained-route policy enforcement. |
| Existing model catalog policy | 4 passed, including startup fallback, configuration changes, and account-transition caching. |
| App-server protocol | 317 passed, 0 failed, 1 ignored. The ignored export-writing helper was invoked explicitly for stable and experimental regeneration. |
| Python SDK contracts | 86 passed: 60 RPC-method, 6 generation-contract, and 20 public-signature tests. |
| Formatting | Workspace Rust formatting check and `git diff --check` passed. Stable rustfmt reports the repository's nightly-only import-granularity setting as unsupported; no formatting diff remains. |
| Clippy | Core, app-server, TUI, and CLI passed with `--tests --offline`, with no warnings in the final run. Narrow serde-predicate lint expectations preserve required reference signatures; test-only existing `unwrap` uses were replaced with diagnostic `expect` calls. |

The selected suites cover **470 distinct tests**. This is not a full workspace run or a production acceptance claim.

## Corrections found during qualification

- Scoped provider defaults were restored after an authority reload instead of importing a default model associated with another provider. A regression includes a higher-priority hidden entry and verifies the visible default.
- Transport-only client-version headers are excluded from durable routing identity, so a binary upgrade does not invalidate a saved route merely by changing its identifying header.
- The generated capability field uses the repository's non-null optional response-field convention.
- The PTY regression waits for a confirmed session rename and complete synchronized terminal updates before checking status, avoiding an early redraw race.
- Static input validation and authoritative native discovery reject unavailable models explicitly. Host-owned provider definitions remain fixed across saved-context restoration.

The sandbox initially rejected local listener creation before integration assertions ran. Local mock tests were rerun outside that sandbox; no live model endpoint was substituted. An early broad core filter also selected an unrelated large async test and overflowed the default stack; the qualified runs use the required 8 MiB stack.

SDK generation used the repository's locked dependencies under a temporary Python 3.12 environment. The pinned generator does not support the system Python 3.14. No user daemon or installed Codex binary was replaced by these checks. Build outputs are under `codex-rs/target/`; shared use requires a compatible rebuilt CLI and daemon.

## Remaining qualification boundaries

- Live Kimi and other real provider/gateway endpoints were not exercised. The caller must supply compatible provider definitions, model metadata, and daemon-owned credentials.
- Native Windows was not built or exercised. The platform-independent Rust code and Linux local-socket path were checked here.
- Catalog-object garbage collection is deliberately absent. Objects are retained rather than deleting data without proving that no saved thread or fork refers to it.
- Managed launchers enforce the recorded reader requirement. An already released, unaware binary manually opening the same store cannot be retroactively made safe by an additive field; unsupported mixed-version access remains an operator constraint.
- First-release fingerprints accept defined public routing fields and host-owned credential references. Ambiguous inline URI credentials or unsupported static header/query forms must be replaced with supported host references or explicitly supported routing fields.

## Astra review remediation (2026-10-27)

The four original review findings and a related concurrent-resume finding were corrected:

- Pending handles are consumed atomically after request/context validation. A failed or cancelled attachment attempt requires fresh resolution. Start, resume, and fork retain authentication ownership through startup, check connection liveness and owner generation before creation and before accepting the result, and shut down/remove newly created runtimes if those checks fail.
- Fork resolution carries the requested destination directory, and attachment preserves that directory. Existing explicit provider selections also remain effective during resolution.
- Authoritative provider-native catalogs are checked when attaching a context and at all three later model-change boundaries: `turn/start`, `turn/settings/update`, and `thread/settings/update`.
- Explicit replacement catalogs are loaded through the trusted snapshot override before initial configuration validation. Current managed requirements still reject a noncompliant replacement; an obsolete or missing previous catalog object cannot block a compliant replacement.
- Resume retains its consumed context across configuration retries and compares it with any newly loaded runtime under the resume permit before taking the running-thread shortcut. Conflicting catalog/model selections fail with `modelContextConflict`.

Astra performed a follow-up source review and confirmed the original four corrections and the additional concurrent-resume correction. That review was read-only and did not execute tests.

Startup futures are boxed before passing through the attachment guard. The broader TUI lifecycle regression initially exposed a stack overflow; the corrected path passes with the repository's 8 MiB test-stack setting. The cold-resume fixture completes a mock turn before restart so the saved thread is discoverable.

Selected requalification passed **17 distinct tests** (overlapping earlier qualification; not an additional full-workspace run):

| Check | Result | Detailed log |
| --- | --- | --- |
| App-server context/authentication/resume unit regressions | 5 passed, including atomic handle consumption, revocation before/after creation, and differing contexts consumed while cold. | `/tmp/mixed-fixes-unit-reviewed.log` |
| Mixed-provider integrations | 6 passed against the final resume correction, including pipelined duplicate starts, native model-change APIs, and managed catalog replacement with a missing previous object. | `/tmp/mixed-fixes-integration-reviewed.log` |
| TUI provider and fork-directory regressions | 3 passed with the 8 MiB stack. | `/tmp/mixed-fixes-tui-delivery.log` |
| Embedded and remote workspace-root lifecycles | 2 passed. | `/tmp/mixed-fixes-roots-delivery.log` |
| Exact Kimi-flags CLI/daemon fixture | 1 passed; the pre-existing daemon PID remained unchanged. Synthetic provider only. | `/tmp/mixed-fixes-cli-delivery.log` |
| Clippy | App-server, TUI, and CLI with `--tests --offline`: passed without warnings. | `/tmp/mixed-fixes-clippy-delivery.log` |
| Formatting and whitespace | Workspace `cargo fmt --all -- --check` and `git diff --check`: passed. | `/tmp/mixed-fixes-format-check.log`, `/tmp/mixed-fixes-diff-check.log` |

The additional unit and TUI commands are:

```sh
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-app-server --lib model_context --offline
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-tui --lib app_server_session::provider_selection_tests --offline
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-tui --lib app_server_session::workspace_roots_tests --offline
```

No live-provider or native Windows qualification was added by this remediation.

## Second Astra review remediation (2026-10-27)

The four additional findings were corrected:

- Core context creation/restoration compares the candidate catalog with bounded, validated content read from the authority-owned required catalog path. Mutating both the effective configuration and proposed snapshot cannot satisfy a mandatory catalog. Role application and scoped thread startup enforce the same check.
- Internal V2 child reload restores the latest child-owned context before reapplying role restrictions, independently of an inherited parent context. Ordinary reload ignores role routing/catalog-source fields while retaining current role restrictions. It restores the saved model and catalog without rereading the provenance file, and still validates host routing and managed requirements.
- Child preparation resolves the target provider/catalog and default model before reasoning-effort and service-tier validation. Those checks use the child's models manager. Legacy child configurations keep their existing selection path.
- Attachment and model-change requests check direct-input authorization where applicable and refresh/enforce current provider/network policy before network-capable native-catalog validation. The native regression verifies an unchanged discovery-request count after policy revokes a provider for attachment, `turn/start`, `turn/settings/update`, and `thread/settings/update`.

Astra's focused follow-up source inspection confirmed these four correction paths and reported no concrete residual defect within that scope. The review itself was read-only and did not rerun tests.

Selected requalification passed **84 distinct tests**, with `RUST_MIN_STACK=8388608`. These counts overlap the earlier qualification sets and do not represent a full-workspace or live-provider run.

| Check | Passed | Detailed log |
| --- | ---: | --- |
| Core context validation and persistence | 8 | `/tmp/mixed-review2-core-context-final.log` |
| Role overrides, including mandatory catalog rejection | 25 | `/tmp/mixed-review2-roles-final.log` |
| Native/static child defaults and child reasoning/tier metadata | 2 | `/tmp/mixed-review2-child-defaults-final.log` |
| V2 child reload after catalog source change/deletion/corruption, with current role restrictions | 3 | `/tmp/mixed-review2-child-reload-final.log` |
| Existing V2 reload behavior | 2 | `/tmp/mixed-review2-legacy-reload-final.log` |
| Existing service-tier inheritance | 2 | `/tmp/mixed-review2-service-tier-final.log` |
| Existing provider delivery | 14 | `/tmp/mixed-review2-provider-delivery-final.log` |
| App-server context/authentication/resume units | 5 | `/tmp/mixed-review2-server-context-final.log` |
| Mixed-provider integrations, including policy-before-discovery assertions | 6 | `/tmp/mixed-review2-integration-final.log` |
| Existing provider APIs and managed requirements | 7 | `/tmp/mixed-review2-provider-api-final.log` |
| Existing catalog policy behavior | 4 | `/tmp/mixed-review2-catalog-policy-final.log` |
| TUI provider/fork-directory behavior | 3 | `/tmp/mixed-review2-tui-provider-final.log` |
| Embedded/remote workspace-root lifecycles | 2 | `/tmp/mixed-review2-tui-roots-final.log` |
| Exact Kimi-flags CLI/daemon fixture, unchanged daemon PID | 1 | `/tmp/mixed-review2-cli-final.log` |

Core, app-server, TUI, and CLI Clippy checks with `--tests --offline` passed without warnings (`/tmp/mixed-review2-clippy-final.log`). Workspace formatting, the final test-file formatting check, and `git diff --check` passed. The initial Clippy run identified an identity conversion in a test, which was removed before the clean final run.

The initial broader core name filter also passed 18 tests, including eight counted above; the selected total does not add those overlapping runs. Local mock integrations ran outside the listener-restricted sandbox, using temporary homes and synthetic credentials. No live Kimi endpoint, native Windows environment, user daemon replacement, commit, or push was part of this remediation.

Additional core regression commands:

```sh
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-core --lib agent::role::tests --offline
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-core --lib agent::child_config::mixed_context_tests --offline
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-core --lib mixed_context_child_reload_restores_owned_catalog_and_current_role_restrictions --offline
```

## Third Astra review remediation (2026-10-27)

The three additional findings were corrected:

- Same-provider model-only roles validate their selection against the static catalog even when reusing a matching context. Core startup enforces the same rule when explicit provider-model fallback is disabled.
- Profile launches retain explicit invocation model, effort, provider, and catalog choices. Resolution and attachment preserve `-m` precedence over `-c model=...` and profile defaults.
- Context compatibility and equality include the API-key discovery setting. Conflicting dotted/nested attachment overrides fail with `modelContextConflict` before discovery; identical settings remain valid. Config loading checks compatibility after runtime feature defaults are applied. Direct core derivation creates an independent manager when inherited settings differ, and loaded-resume comparisons include the setting.

Astra's focused follow-up source review confirmed these three correction paths and reported no concrete residual issue within that scope. It was read-only and did not execute tests.

Selected requalification passed **92 distinct tests**, with `RUST_MIN_STACK=8388608`. Counts overlap previous remediation runs and do not constitute full-workspace qualification.

| Check | Passed | Detailed logs |
| --- | ---: | --- |
| Core context, static membership, and isolated manager configuration | 10 | `/tmp/mixed-review3-core-context-final.log` |
| Role overrides, child settings/defaults, and catalog reload | 31 | `/tmp/mixed-review3-roles-final.log`, `/tmp/mixed-review3-child-defaults-final.log`, `/tmp/mixed-review3-child-reload-final.log` |
| Existing V2 reload, service-tier, and provider delivery behavior | 18 | `/tmp/mixed-review3-legacy-reload-final.log`, `/tmp/mixed-review3-service-tier-final.log`, `/tmp/mixed-review3-provider-delivery-final.log` |
| Runtime-feature binding, server contexts, mixed integrations, provider APIs, and catalog policy | 23 | `/tmp/mixed-review3-runtime-binding-final.log`, `/tmp/mixed-review3-server-context-final.log`, `/tmp/mixed-review3-integration-final.log`, `/tmp/mixed-review3-provider-api-final.log`, `/tmp/mixed-review3-catalog-policy-final.log` |
| TUI launch choices, provider selection, and workspace-root lifecycles | 8 | `/tmp/mixed-review3-tui-launch-final.log`, `/tmp/mixed-review3-tui-provider-final.log`, `/tmp/mixed-review3-tui-roots-final.log` |
| Profile model override and shared-daemon CLI fixtures | 2 | `/tmp/mixed-review3-cli-profile-final.log`, `/tmp/mixed-review3-cli-final.log` |

The native integration tests verify that conflicting nested/dotted discovery opt-outs perform no provider requests, leave the existing context usable, and permit an identical discovery setting. The runtime-binding unit test starts with an explicit runtime opt-out and verifies compatibility after defaults are applied, then rejection after the runtime setting changes. Discovery is enabled by default; an initial isolation fixture assumption was corrected to set its initial state explicitly before the passing rerun.

The PTY profile fixture defines a different profile-default model, launches `codex --profile work -m k3.1-pro`, confirms a running named session, and checks its status. The separate shared-daemon fixture again verifies the motivating Kimi flags and unchanged daemon PID. Both fixtures use synthetic provider configuration.

Core, app-server, TUI, and CLI Clippy checks with `--tests --offline` passed without warnings (`/tmp/mixed-review3-clippy-final.log`). Final workspace formatting and `git diff --check` passed. No live-provider or native Windows qualification was added.

Additional targeted commands:

```sh
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-app-server --lib context_binding_checks_discovery_after_runtime_feature_defaults --offline
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-tui --lib app_server_session::startup_launch::mixed_provider_tests --offline
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-cli --test daemon_startup profile_launch_preserves_the_explicit_model_flag --offline -- --exact
```

## Reproduction

Run from the repository root with the pinned compiler (or `+stable` only when it is the same compiler version):

```sh
RUST_MIN_STACK=8388608 cargo +stable check --manifest-path codex-rs/Cargo.toml -p codex-cli -p codex-app-server -p codex-tui --tests --offline
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-core --lib model_context::tests --offline
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-app-server --lib model_context::tests --offline
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-app-server --test all suite::v2::mixed_provider_app_server --offline
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-cli --test daemon_startup mixed_provider_flags_attach_to_the_existing_shared_server --offline -- --exact
RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-app-server-protocol --lib --offline
RUST_MIN_STACK=8388608 cargo +stable clippy --manifest-path codex-rs/Cargo.toml -p codex-core -p codex-app-server -p codex-tui -p codex-cli --tests --offline
cargo +stable fmt --manifest-path codex-rs/Cargo.toml --all -- --check
```

Integration tests need permission to bind local mock HTTP and Unix-socket listeners. The API and PTY fixtures use temporary homes and synthetic credentials. Temporary detailed logs are under `/tmp/mixed-*.log`; the durable record here captures commands, counts, scope, and limitations without copying credentials or user configuration.

## Mixed-provider `/status` correction (2026-10-28)

The TUI retains the host account identity while scoping `/status` account details, usage links, cached limits, and billing requests to the active OpenAI account-backed provider. Start, resume, and fork preserve the resolver's authentication requirement in the internal thread session state. Cached session switches restore that requirement; incomplete metadata for a different provider does not inherit the previous provider's requirement. Account updates invalidate cached limits and outstanding account-scoped status requests.

Validation used the pinned **Rust 1.99.1** compiler. The installed `stable` alias currently selects 1.96.0, so the earlier document's historical equivalence between `+stable` and the compiler pin does not apply to this rerun. The initial stable build and the fully optimized pinned build were interrupted during compilation and provide no runtime validation. The completed test build disabled optimization only for the TUI package and retained the compiled release dependencies:

```sh
RUST_MIN_STACK=8388608 cargo +1.99.1 test --manifest-path codex-rs/Cargo.toml \
  -p codex-tui --lib --release --offline \
  --config 'profile.release.package.codex-tui.opt-level=0' <filter>
```

| Filter | Passed |
| --- | ---: |
| `chatwidget::tests::app_server::status_` | 2 |
| `chatwidget::tests::status_command_tests` | 16 |
| `app_server_session::provider_selection_tests` | 4 |
| `app::tests::thread_usage` | 8 |
| `session_and_settings_sync_server_provider_id` | 1 |
| `app::thread_session_state::tests` | 4 |
| `app_server_session::workspace_roots_tests` | 2 |

These **37 distinct selected tests passed**. The three new regressions cover provider switches and account notifications, same-provider authentication changes, and retaining the host identity during custom-provider bootstrap. The existing provider integration also checks authentication metadata through start, resume, and fork. Workspace formatting with the pinned compiler and `git diff --check` passed.

The broader `status_` selection passed 388 tests and failed 22 with local mock listeners allowed. The pre-existing test binary `codex_tui-aa5d2a8ba79c4123` passed 385 tests and failed the exact same 22 named tests; the current selection adds the three new passing regressions. Those failures concern version/shortcut snapshots and an existing Daybreak assertion. This comparison uses an existing binary, not a fresh baseline build of HEAD. The original sandboxed selection passed 387 and failed 23; mock-listener denial prevented one additional test from passing. The remote workspace-root fixture also required a rerun with local listeners allowed, after which both lifecycle tests passed.

Detailed logs are retained under `/tmp/mixed-status-*.log`, including the interrupted builds and broader failures. Generated failing snapshots were moved to `/tmp/mixed-status-snapshot-failures/` without accepting them. No live provider, installed binary replacement, or fully optimized release qualification was performed.

## GPT session-switch latency correction (2026-10-29)

Runtime logs showed `modelContext/resolve` awaiting optional GPT catalog discovery for
about five seconds before starting `thread/resume`. For example, resolution started at
02:07:43.229 UTC, the catalog request timed out at 02:07:48.296, and resume started at
02:07:48.330. Independent/restored contexts previously disabled all catalog caching.

ChatGPT-authenticated OpenAI resume now uses cached or bundled metadata and schedules
optional discovery in the background after publishing the validated pending context.
Initial selection, API-key discovery, command authentication, and authoritative explicit
provider catalogs retain their synchronous discovery rules. Context managers remain
independent while sharing a bounded process-local cache by the existing provider/auth
identity digest. The registry retains 32 scopes, entries expire after five minutes,
client versions must match, and a changed account cannot write into its previous cache.
Static snapshots and API-key discovery opt-outs remain authoritative.

Selected local qualification with Rust 1.99.1 and local mock listeners allowed:

| Check | Result |
| --- | --- |
| `codex-model-provider --lib` with the unrelated Bedrock fixture below excluded | 110 passed, including four new cache regressions for reuse/isolation, expiry/version/identity checks, conditional revalidation, and bounded retention. |
| `codex-app-server --test all suite::v2::mixed_provider_app_server` | 8 passed, including the new stalled-catalog resume regression and existing native-catalog, owner-isolation, and cold-resume cases. |
| `codex-core --lib model_context::tests` | 10 passed, including discovery-setting isolation, managed catalog authority, restored metadata, and provider/tenant fingerprints. |
| Before-fix comparison | The same stalled-catalog regression fails at its two-second navigation deadline against the previously packaged CLI. The fixed app-server passes while discovery remains pending against a 60-second mock response. |
| Formatting | Workspace `cargo fmt --all -- --check`, a final targeted rustfmt check, and `git diff --check` passed. Stable rustfmt reports the existing nightly-only import-granularity setting as unsupported. |

The unfiltered provider run also reports the unchanged
`amazon_bedrock::runtime_catalog::tests::runtime_catalog_disables_web_search_without_overriding_review_models`
fixture failure: its expected vector contains duplicate GPT-6 Sol V1 entries absent from
the runtime catalog. That test and the Bedrock implementation were not changed. The
final provider command explicitly excludes this one test:

```sh
env -u CODEX_SANDBOX_NETWORK_DISABLED RUST_MIN_STACK=8388608 \
  cargo test --manifest-path codex-rs/Cargo.toml -p codex-model-provider --lib --offline --locked -- \
  --skip amazon_bedrock::runtime_catalog::tests::runtime_catalog_disables_web_search_without_overriding_review_models
env -u CODEX_SANDBOX_NETWORK_DISABLED RUST_MIN_STACK=8388608 \
  cargo test --manifest-path codex-rs/Cargo.toml -p codex-app-server --test all \
  suite::v2::mixed_provider_app_server --offline --locked
env -u CODEX_SANDBOX_NETWORK_DISABLED RUST_MIN_STACK=8388608 \
  cargo test --manifest-path codex-rs/Cargo.toml -p codex-core --lib \
  model_context::tests --offline --locked
```

Timing excerpts and the expected-failure comparison log are under
`/tmp/codex-agent-switch-diagnosis/`. The comparison uses the pre-fix packaged CLI
(`bin/codex` SHA-256 `a5a77fd904dbadad647fdac792a4977b53e3800a555ee3c082c1b4d75d21a587`)
through a CLI-compatible test launcher; it is not a fresh baseline build. These are
local synthetic checks, not a live-provider or full terminal-navigation benchmark.
No installed CLI, existing package, or running daemon was replaced.

### Rebuilt release package

The GNU Linux x86-64 CLI was rebuilt with Rust 1.99.1 using the locked, offline
`release` profile from `ad437485bc09eeedb4e2ad5921a03905c41b4b7e` plus this latency
fix. The eight changed Rust files match the source hashes retained in the build
receipt. The existing code-mode host was reused after confirming that its dependency
graph contains none of the changed crates.

- Package: `codex-rs/target/packages/codex-linux-session-switch-fix.tar.gz`
  (140,736,993 bytes, manifest version `0.164.0`).
- Archive SHA-256: `09218447ce57efadcdf12e58610c0c6a9686c61aaa9797712831f7b7c6e26e6f`.
- CLI SHA-256: `0fda416f7636af75cb0274099bf984268dc2ef481248710ebbeac02e484f0da9`.
- Reused host SHA-256: `4afe557d6430b2417a4facb7ff626308f7058c00a425b6e80a8e32e16cd39928`.
- Retained build provenance and verification: the matching `.build.json`,
  `.source.patch`, `.build.log`, `.package.log`, `.receipt.json`, `.tar.gz.sha256`,
  and `.smoke/` artifacts under `codex-rs/target/packages/`.

Archive contents, executable modes, input hashes, and dynamic library resolution
passed verification. The packaged CLI reported `codex-cli 0.164.0`; the packaged
host executed `text(6 * 7)` and returned `42` before clean shutdown. A local mock
Responses flow exercised the packaged CLI and host together, running the package's
own ripgrep successfully under `workspace-write` sandboxing. The same stalled-catalog
regression also passed against the packaged release CLI within its two-second
navigation deadline. The archive checksum was independently checked afterward.

These artifacts remain local build outputs. No running daemon or installed CLI was
replaced, and no live model endpoint was used for release smoke checks.

## Integration into NzSN main (2026-10-29)

The merge combines fork main `00673c3536c324c58df7809f8e37a6408a2b3707` with
`multi_agent_v2_custom_provider` at `9035091d37ef281643977178f104e45c94eb0cea`.
Both histories are retained. The merge was prepared in an isolated worktree and
completed without conflicts.

Before adding this record, the entire merged tree differed from the feature branch
only in `.github/workflows/README.md` and `.github/workflows/rust-release.yml`.
Those two files match fork main exactly, preserving its GitHub-only Linux x86-64
release policy. The `codex-rs` tree is identical to the feature branch, with tree
object `227ca84d23a6c41210d0aef450542b92e271ad1f`; the SDK and remaining build inputs
also retain the feature branch's bytes. This preserves the full NzSN runtime
implementation, including plaintext collaboration, provider-specific roles and
delivery, payload restoration/bounds, provider-bound review, mixed-provider status,
and the session-switch fix.

After this identity audit, the matching cached native test binaries were rerun with
temporary homes and local mock listeners allowed:

| Filter | Passed |
| --- | ---: |
| `plaintext` | 19 |
| `inter_agent_request_projection` | 7 |
| `multi_agent_task_payload` | 3 |
| `provider_delivery_tests` | 14 |
| `agent::role::tests` | 25 |
| `model_context::tests` | 10 |
| `mixed_provider_app_server` | 8 |
| `mixed_provider_auto_review` | 6 |

All **90 distinct selected tests passed**; overlapping filters are counted once.
Each filter was checked against the test listing before execution, so an empty
selection could not be accepted. Logs and the per-filter inventory are retained in
`.cache/main-merge-validation/`. This is an identity-backed rerun of matching cached
binaries, rather than a fresh build in the merge worktree. The previously verified
GNU Linux package has the same runtime source tree; the fork's musl release workflow
was preserved but was not executed as part of this local merge qualification.

## Upstream rust-v0.165.0 integration (2026-10-29)

The upstream release is an annotated tag rather than a remote branch. Origin's tag
object is `e374836b2728d9b7aa981c2902649e640a2edaa8`, resolving to commit
`360319d8dee62d87e2d59fc0c21805d79519cfed`. The merge starts from NzSN main
`71a2aeed7c94dfcff40b5b3ba883ac4a47ce5795`; both histories are retained. Workspace
and path-package lockfile versions are updated to `0.165.0` without refreshing
unrelated dependency selections.

Nine conflicts were resolved by combining behavior:

- Keep the fork's GitHub-only Linux release workflow. Upstream's conflicting edits
  concern Python runtime wheels, which that fork workflow does not publish. The
  workflow remains byte-identical to the pre-merge fork, and its symbol-stripping
  policy remains intact.
- Retain mixed-provider documentation alongside upstream's new memory-summary API.
  Register both the new Vertex adapter and NzSN's context cache module.
- Select models from the retained provider/catalog context before applying upstream's
  originator scoping. The scoped manager is rebound into the context and runtime
  together, preserving the approval-review pointer-identity guard. Custom-provider
  wrappers delegate scoping; immutable static catalogs remain authoritative.
- Partition context caches by provider/auth identity and originator, including when
  the default and scoped endpoints have the same identity. Cache capacity, TTL,
  current authentication checks, and discovery opt-outs are retained.
- Preserve explicit provider/catalog/profile launch choices while adding upstream's
  invocation-only approval-review defaults. A selected profile retains its reviewer
  when there is no invocation override; an explicit invocation reviewer wins.
- Retain upstream's native reviewer for the newly supported built-in GCP ID while
  custom provider IDs, including Kimi, still use their captured parent model.
- Regenerate stable/experimental protocol exports and Python SDK types from the
  merged Rust definitions. Retain upstream's corrected Bedrock catalog expectations
  rather than reintroducing stale duplicate V1 rows. Review the changed model-menu
  snapshot hash after the upstream Terra catalog addition.

The fork-only preservation inventory contains 190 paths: 180 retain their exact
pre-merge bytes, while ten code/test/documentation paths receive the originator
integration and its validation record. Shared upstream/fork files are checked by
the targeted regression suites rather than resolved by preferring one entire side.

Fresh Rust 1.99.1 builds compile the merged sources; the prior 0.164.0 test binaries
are not used for this qualification. Selected runtime results comprise **646 distinct
passing Rust tests**, with overlapping filters counted once:

| Area | Passed |
| --- | ---: |
| Complete app-server protocol unit suite | 317 |
| Complete model-manager unit suite | 56 |
| Complete provider unit suite, including Vertex and originator caches | 124 |
| Core context, originator, plaintext, projection, payload, delivery, role, and review units | 84 |
| App-server context, mixed-provider, review, and attached-thread checks | 24 |
| Core plaintext wire/scheduling, catalog parameters, and originator instruction scenarios | 8 |
| TUI launch, provider selection, status, and startup defaults | 33 |

The protocol suite has one ignored schema-writing helper, invoked separately for
both stable and experimental regeneration. The canonical config-schema generator
matches the merged fixture apart from its existing final newline. Python SDK types
were regenerated with locked datamodel-code-generator `0.64.0`; managed Python 3.12
download attempts timed out, so generation used Python 3.14.4 and the same locked
dependency versions in an isolated environment.

Affected core, app-server, TUI, CLI, provider, and model-manager crates passed
`cargo check --tests --locked --offline` and `cargo clippy --tests --locked --offline`;
the final Clippy run emitted no warnings. Workspace Rust formatting and whitespace
checks passed. Every selected Rust group was checked against its nonempty test
inventory and successful summary, including TUI tests that write terminal escapes
around individual result lines.

SDK validation records **268 passes, 38 opt-in real-Codex skips, and one unchanged
upstream failure**. `test_root_format_driver_covers_all_formatter_groups` expects
direct `rustfmt`, while the upstream `scripts/format.py` invokes `cargo fmt`. The
failure was reproduced independently with both the test and driver copied directly
from the upstream tag. Neither file was changed to hide the mismatch. Initial
packaging-fixture failures due to `uv` missing from PATH passed after correcting
the test environment.

A cross-version fixture creates a durable catalog-backed plaintext conversation
with the verified 0.164.0 package, removes its original catalog JSON and explicit
payload-mode setting, and cold-resumes it with the current 0.165.0 app-server.
The model, provider, and catalog digest survive. A subsequent cold resume with an
explicit encrypted-mode override is rejected. A current 0.165.0 development host
also executes `text(6 * 7)` over stdio, returns `42`, and shuts down cleanly.

Detailed build logs, per-filter inventories, schema-generation records, the upstream
formatter baseline, and cross-version receipts are retained under
`.cache/upstream-0.165-merge/`. These are selected local checks, not a full workspace,
native-Windows, Bazel, live-provider, or release-package qualification. The existing
0.164.0 package and running daemon were not replaced by this source merge.

## Upstream rust-v0.166.0 integration (2026-10-31)

This integration starts from NzSN main `3100dfe51f1858dbc8c12b19604f3bcb63573fb5`
and merges upstream `rust-v0.166.0`, whose tag object
`593389380740f7b4762771573c447ccfdc59742f` resolves to
`6ff1e1ca3b30fc8b914720681fcf5fe62e95f856`. The actual merge base is
`b559e6a3988489b636e1e79672eff330bf0679dc`. Both parent histories are retained.
The fork release tag is `rust-v0.166.0.0`; Cargo workspace and local package
lockfile versions remain the valid upstream Rust version `0.166.0`.

Conflict resolution combines these behaviors:

- Preserve provider-bound model contexts, catalog authority, originator scoping,
  runtime-manager identity checks, and the custom-provider parent reviewer. The
  provider catalog wrapper delegates upstream's new catalog-with-origins API;
  the scoped-context regression also checks the selected model's provider origin.
- Retain per-thread provider routing across daemon default changes. Upstream's
  single-selected-provider equality rule conflicts with concurrent mixed-provider
  clients, so admission keeps managed-policy and missing-definition validation
  without imposing daemon-default equality. Regression cases cover retained routes,
  missing-provider recovery, and policy failures before inference traffic.
- Keep the GCP synchronous reviewer and the fork's GCP async-classifier exclusion,
  alongside upstream's reviewer prompt preparation and bounded catalog refresh.
- Preserve plaintext collaboration, provider-specific roles, evicted delivery
  policies, payload-mode locking, and standalone custom-provider tool outputs.
  Upstream separates child preparation from message delivery; plaintext size
  validation remains before child allocation, while completion-envelope checks
  stay in preparation. New tool-router fixtures include the fork's message policy.
- Adapt running-tool tracking, replay, presentation, and resume redaction to
  upstream's timestamped item envelopes. Preserve item IDs, available start times,
  parallel activity, completion precedence, and activity independent of history
  pagination and buffer eviction. Redaction changes payloads while retaining the
  envelope. The renamed Astra plugin snapshot retains its prior bytes.
- Retain explicit model/catalog/profile launch choices and reviewer precedence
  while incorporating upstream's projectless approval defaults and server-owned
  settings. Profile launches retain their resolved approval policy when no
  invocation policy is provided, and structured invocation overrides survive the
  profile path. Both cases have regression assertions.
- Regenerate stable and experimental protocol exports and Python SDK models from
  the merged Rust definitions. Update retained account fixtures for upstream's
  optional workspace fields.
- Keep GitHub-only Linux x86_64 musl releases on `ubuntu-24.04`, fourth-component
  tag validation, and sidecar-symbol archival followed by stripping. Add upstream's
  newly bundled `codex-exec-server` to the fork's build and symbol-archive inputs.

The provider-role implementation and tests and the orphaned-stream integration
suite retain their pre-merge bytes. Their combined stable patch ID against
`rust-v0.165.0` remains `f2dc82c38aa965522b6c2daaf92e29e83edbfda5`.
The preservation inventory and per-suite test listings are retained under
`.cache/upstream-0.166-merge/`.

Validation uses Rust 1.99.1 and unoptimized native GNU Linux binaries. The new
upstream V8 Git dependency attempts to fetch native Chromium submodules even for
prebuilt builds. Local validation instead uses `git archive` of the exact pinned
Rust source commit `350c5a859a9b3c84ab8701b3ad51e3b7699b30c0`: all 163 archived blobs
were verified against Git object IDs. The archive and binding inputs match the
checked-in trusted manifest, whose SHA-256 is
`1ec730635970ab1c0e8f4764ea113ad29d69d7f8d4c2ae69f3d766434a759350`.
The local path substitution is confined to validation; the published manifest and
lockfile retain upstream's Git dependency identity. Native V8 source-build
submodules are outside this local validation. Async development fixtures use a
32 MiB test-thread stack; the initial default-stack overflow receipt is retained.

Final runtime checks passed **705 distinct selected Rust tests**:

| Area | Passed |
| --- | ---: |
| Complete app-server protocol unit suite | 322 |
| Complete provider unit suite | 136 |
| Complete model-manager unit suite | 58 |
| Core role, context, originator, plaintext, payload, projection, delivery, review, and moved dispatch tests | 95 |
| App-server context, persisted resume, redaction, retained-route, and provider-policy units | 25 |
| Core plaintext wire/scheduling and orphaned-stream integrations | 6 |
| Mixed-provider, review, managed-policy, and provider-definition/recovery integrations | 26 |
| TUI profile/provider startup, activity hydration, account scoping, and update-message snapshot | 37 |

The protocol suite has one ignored generation helper, invoked separately for both
stable and experimental exports. Every selected runtime filter was checked against
a nonempty compiled inventory. Core, app-server, TUI, and CLI test targets passed
type checking; the final app-server/TUI unit and integration targets were rebuilt
following the provider-admission repair. The native CLI, app-server, code-mode host,
standalone executor, sandbox, and config-schema generator built successfully.
Workspace Rust formatting and whitespace checks passed. The generated config
schema matches the merged fixture semantically. Existing upstream compiler and
vendored-C warnings remain visible in the logs.

The complete SDK run against the final merged native CLI passed **273 tests**, with
**38 opt-in cases skipped**. Its external-input checks verify custom-provider
synthetic call/output pairing, namespace authority, and stable call IDs across
resume. Release/tag helpers passed 29 tests and package-layout/archive helpers
passed 27 tests.

Intermediate receipts are retained: the first mixed-provider integration run had
eight provider-admission failures, repaired before the expanded 26-test run; the
initial SDK run without its runtime had 46 setup failures; and the first native
SDK run exposed three expected-wire mismatches with the fork's previously added
call pairing. The final SDK run includes the corrected pairing assertions. Only
the inspected update-message snapshot was accepted, retaining upstream's new
installer command and replacing its Bazel placeholder version with Cargo 0.166.0.

These are selected native GNU Linux source checks. Full-workspace, Bazel,
native-Windows, live-provider, native V8 source-build, and musl release-package
qualification were not run locally. The requested hosted release build uses the
published tag, restored upstream dependency identity, and the fork's existing
GitHub-only Linux musl workflow.
