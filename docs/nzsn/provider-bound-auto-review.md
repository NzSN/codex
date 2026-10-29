# Provider-bound automatic approval review

Date: 2026-10-28. Status: implemented with selected local qualification passed.

## Problem

A legacy Kimi thread resumed through the mixed-provider app-server with its provider and
`approvals_reviewer = "auto_review"` intact, but without a saved model catalog. The model
manager's bundled OpenAI metadata then advertised `codex-auto-review`. Guardian selected
that model on the Kimi endpoint and constructed an incompatible freeform `exec` tool.
The endpoint rejected the review, so the pending action was not executed.

There is a second independent path: Guardian V2's optional asynchronous classifier uses
a fixed Luna model. Merely fixing synchronous selection would leave that path able to
send an OpenAI model name to a custom provider.

## Required behavior

For a Kimi task using `k3.1-pro`, automatic approval runs in an isolated, read-only
reviewer using **Kimi / k3.1-pro**, the task's provider configuration and authentication,
and its own catalog metadata. Resume, restart, another client's default provider, or an
OpenAI login must not change that selection. The reviewer remains a separate session
with the existing Guardian policy, structured decision contract, and authorization checks.

## Design

1. **Make the provider selection policy explicit.** OpenAI and the built-in Bedrock
   and GCP (Vertex) adapters retain their backend-specific reviewer selection. Other provider
   IDs, including Kimi and local providers, use the action-time parent model. Their
   reviewer selection ignores bundled preferred models and `auto_review_model_override`;
   it retains the parent model metadata and supported reasoning settings. This policy
   is shared by core and the Guardian extension, rather than inferred from login state,
   display names, or whether a catalog happens to contain an OpenAI model slug.

2. **Require provider-owned metadata before a custom-provider review.** A resolved
   model context must match the thread's provider/configuration and reference the
   exact runtime model manager. Its static catalog must contain the active model,
   or its explicit native catalog must provide non-fallback metadata. A catalog
   path, URL, or matching model name alone is not proof of ownership. A legacy
   thread without this binding may resume normally, but its automatic review
   fails before reviewer startup or provider I/O with instructions to rebind
   `model_catalog_json` or configure `model_catalog_url`. Do not guess a catalog filename,
   change user configuration, disable approval, or silently use bundled OpenAI metadata.
   Standalone custom-provider startup catalogs establish a validated default binding;
   only matching provider/catalog configurations can inherit that binding. Legacy
   per-thread overrides need explicit context resolution rather than inheriting the
   daemon's manager. The startup binding excludes the selected model; model membership
   is checked at thread start after per-thread overrides have been applied. A stale
   daemon default therefore cannot invalidate a valid explicit thread selection.

3. **Use the existing durable model-context mechanism for recovery.** Explicit
   `modelContext/resolve` with resume intent and a catalog can replace the empty legacy
   context. Attach that handle through `thread/resume`; the validated catalog snapshot
   is then persisted. Subsequent resumes work even if the original catalog file is
   gone. Current managed requirements and ownership checks remain authoritative.

4. **Keep custom-provider review synchronous.** The built-in Luna scorer and optional
   external Decisions comparison are specialized backends, not general provider-neutral
   reviewers. Do not initialize them for custom-provider threads, even when Guardian V2
   is enabled globally. Clear stale scoring state on initialization and prohibit score
   reuse for such threads; their approval decisions go through the ordinary synchronous
   reviewer. Initialize shared evidence recording and trusted skill roots before this
   guard so delegated workers can still receive verified root skill paths. Building
   a provider-neutral background classifier is outside this fix.

5. **Construct tools from the selected metadata.** Reuse the task's provider-bound
   catalog and model rather than borrowing the dedicated OpenAI reviewer's metadata.
   Existing tool capability and managed-feature enforcement remains in effect. Kimi's
   direct-tool catalog must produce function tools, without an injected custom `exec`.
   If the reviewer fails or returns invalid output, the action remains blocked.

No new public protocol fields, credential storage, provider endpoint defaults, or
configuration switches are required. Built-in adapter behavior is preserved. The GCP
exception was added with the upstream 0.165.0 integration; custom provider IDs still
review with their captured parent model.

## Validation and acceptance

- A local mock Kimi endpoint handles a real escalation review, using `k3.1-pro` and
  compatible tools; verify both allow and deny results affect command execution.
- Repeat after app-server restart and after the original static catalog file is removed.
- Exercise a legacy thread with no durable catalog: no reviewer model request is made,
  the action remains unexecuted, and the error names the catalog recovery option.
- Rebind an explicit Kimi catalog to that legacy thread and verify review succeeds.
- With the same model name in two catalogs, reject an unbound inherited catalog or
  legacy per-thread catalog override; an explicit rebind must use Kimi's review policy.
- Preserve review from a standalone Kimi startup catalog without requiring a client handle.
- Accept a valid explicit thread model when the daemon default has been removed from
  that catalog, while still rejecting the stale default and other unknown selections.
- Record trusted skill paths on fresh OpenAI and Kimi roots, independently of classifier setup.
- Keep a separate default provider active and verify Kimi review traffic never reaches it.
- Enable Guardian V2 and its comparison feature for a Kimi thread; assert no Luna or
  Decisions sampler is initialized and synchronous Kimi review still completes.
- Retain existing OpenAI reviewer-selection, mixed-provider, and approval-policy tests;
  run formatting and targeted Clippy checks.

These are deterministic local tests with synthetic credentials. They do not qualify
the live Kimi service or replace the installed daemon.

## Implementation and validation record

- [Provider selection policy](../../codex-rs/model-provider/src/provider.rs) identifies
  the built-in adapters explicitly. [Core reviewer resolution](../../codex-rs/core/src/guardian/reviewer_config.rs)
  selects the captured parent model for custom providers without listing the bundled
  catalog, and checks the resolved configuration and runtime manager identity before
  preparing an automatic review. [Thread startup](../../codex-rs/core/src/thread_manager.rs)
  retains a validated startup binding for matching legacy configurations, without
  treating per-thread catalog overrides as proof of ownership.
- [Parent-model selection](../../codex-rs/ext/guardian-reviewer/src/model.rs) retains
  supported reasoning settings and ignores a stale preferred-model override.
- [Classifier initialization](../../codex-rs/ext/guardian-v2/src/async_scorer/extension.rs)
  initializes shared trusted-skill evidence before removing custom-provider classifier
  state. [Approval routing](../../codex-rs/ext/guardian-v2/src/async_scorer/approval.rs)
  prevents cached scores from authorizing or racing their synchronous reviews.
- [Mixed-provider approval regressions](../../codex-rs/app-server/tests/suite/v2/mixed_provider_auto_review.rs)
  execute temporary marker commands, verify allow/deny behavior and outgoing model/tool
  declarations, exercise cold resume after catalog deletion, and repair a legacy
  session through an explicit catalog rebind. They enable Guardian V2 and Decisions
  comparison to ensure those settings cannot redirect Kimi review.
- Existing OpenAI integration fixtures now select the built-in OpenAI identity instead
  of the custom ID `mock_provider`. Their mock transport accepts classifier WebSockets
  and negotiates HTTP fallback for actor/reviewer requests; request compression is off
  because those handlers consume JSON directly.

Selected checks use Rust 1.99.1 and `RUST_MIN_STACK=8388608`. Network tests run with
local listeners permitted and the inherited `CODEX_SANDBOX_NETWORK_DISABLED` test-skip
marker removed. No live provider credentials or user sessions are used by the tests.

### Initial qualification

| Check | Result | Local log |
| --- | --- | --- |
| Core Guardian unit and request tests | 59 passed | `/tmp/provider-bound-review-core-tests.log` |
| Mixed-provider integration, including Kimi approval | 9 passed | `/tmp/provider-bound-review-mixed-final.log` |
| Managed automatic-review policies | 4 passed | `/tmp/provider-bound-review-policy-tests.log` |
| Persisted reviewer restoration | 1 passed | `/tmp/provider-bound-review-resume-test.log` |
| Existing app-server Guardian integration | 111 distinct passed across the suite and code-mode rerun | `/tmp/provider-bound-review-app-guardian-tests.log`, `/tmp/provider-bound-review-code-mode-tests.log` |
| Permission grants and strict review | 8 passed | `/tmp/provider-bound-review-permission-tests.log` |
| Custom-provider classifier state | 1 passed | `/tmp/provider-bound-review-classifier-test-final.log` |
| Targeted Clippy with warnings denied | Passed | `/tmp/provider-bound-review-clippy-final.log` |

The total is **193 distinct passing tests**. The initial broad Guardian run had
101 passes and ten failures caused by the missing code-mode host executable. After
building it, all eleven code-mode cases passed (one overlaps the initial passes).
Rust formatting, design-link validation, and `git diff --check` also passed. This is
selected local qualification, not full-workspace or live-provider acceptance.

### Repeat-review remediation

Two additional probes exposed gaps not covered by the initial qualification:

- A legacy request with a custom provider, an explicit catalog override, and a model
  name also present in the daemon's catalog could pass the catalog-presence check while
  actually using the daemon's review policy.
- The custom-provider early return skipped trusted skill-root initialization, so a
  fresh Kimi root could not record verified skill paths for delegated reviewers.

Both are fixed. Runtime validation now requires a matching resolved context and the
same model-manager instance used by the session. An unbound legacy request is rejected
before reviewer I/O, even if it supplies a catalog path or matches a default model name.
Explicit rebinding restores Kimi's policy; matching standalone startup catalogs obtain
a valid binding automatically. Shared evidence recording runs before classifier gating.

That requalification passed **202 distinct tests**:

| Check | Passed | Local log |
| --- | ---: | --- |
| Core Guardian, including wrong-manager/configuration rejection | 59 | `/tmp/provider-review-fixes-core.log` |
| Model-context ownership, discovery, persistence, and managed policy | 10 | `/tmp/provider-review-fixes-context.log` |
| Mixed providers, including both same-name legacy cases and startup compatibility | 12 | `/tmp/provider-review-fixes-mixed-final.log` |
| App-server Guardian, including root-skill delegation | 111 | `/tmp/provider-review-fixes-app-guardian.log` |
| Permission grants and strict review | 8 | `/tmp/provider-review-fixes-permissions.log` |
| Custom-provider classifier state and fresh-root skill recording | 2 | `/tmp/provider-review-fixes-skills.log` |

Clippy with warnings denied passed (`/tmp/provider-review-fixes-clippy-final.log`).
Formatting, documentation links, and `git diff --check` passed. Counts overlap the
initial qualification and are not additive. No installed daemon was replaced, and
live-provider/native Windows qualification remains open.

### Stale-default model remediation

A subsequent review reproduced a valid legacy `thread/start` model override losing
automatic review when the daemon's default model was absent from its static catalog.
Default-context construction had validated that unrelated default selection and then
discarded the entire binding on failure.

Default-context construction now uses a configuration copy with no selected model.
Provider, catalog, route, and managed-policy validation remain intact. Thread startup
still validates the effective model after request overrides are applied; the original
daemon configuration is not modified.

Focused requalification passed **23 tests**: 13 mixed-provider cases, including the
new stale-default regression, and ten model-context cases. The new regression also
checks that omitting the override or explicitly choosing another unknown model is
rejected, while a valid `k3.1-pro` override completes a Kimi review and executes the
approved temporary marker command using Kimi's policy.

Logs: `/tmp/provider-stale-default-mixed.log`, `/tmp/provider-stale-default-context.log`.
Clippy with warnings denied passed (`/tmp/provider-stale-default-clippy.log`), as did
formatting, documentation-link validation, and `git diff --check`. These focused checks
overlap earlier qualification; they are not an additional full-suite run or live-provider
acceptance. Deployment remains separate.

The broader integration suite requires `codex-code-mode-host` alongside the test
app-server binary. Build it before running that suite. This environment has matching
V8 152.2.0 artifacts in `.cache/rusty-v8`; supply `RUSTY_V8_ARCHIVE` and
`RUSTY_V8_SRC_BINDING_PATH` when building offline.

Representative reproduction commands, from the repository root:

```sh
cargo +stable build --manifest-path codex-rs/Cargo.toml -p codex-code-mode-host --offline
env -u CODEX_SANDBOX_NETWORK_DISABLED RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-core --lib guardian::tests --offline
env -u CODEX_SANDBOX_NETWORK_DISABLED RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-app-server --test all suite::v2::mixed_provider --offline
env -u CODEX_SANDBOX_NETWORK_DISABLED RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-app-server --test all suite::v2::guardian_v2 --offline -- --test-threads=4
env -u CODEX_SANDBOX_NETWORK_DISABLED RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-core --lib model_context::tests --offline
env -u CODEX_SANDBOX_NETWORK_DISABLED RUST_MIN_STACK=8388608 cargo +stable test --manifest-path codex-rs/Cargo.toml -p codex-guardian-v2 --lib custom_ --offline
RUST_MIN_STACK=8388608 cargo +stable clippy --manifest-path codex-rs/Cargo.toml -p codex-model-provider -p codex-guardian-reviewer -p codex-core -p codex-guardian-v2 -p codex-app-server --tests --offline -- -D warnings
```

For a legacy Kimi session, first unload the thread (or restart the daemon), then
supply `-c 'model_catalog_json="~/.codex/models_kimi.json"'` when launching resume.
That explicitly replaces the catalog-less context; later resumes use its persisted
snapshot. The existing API rejects changing an already loaded thread's model context
in place. The running installed daemon must be updated to the fixed build before it
can enforce the new reviewer-selection policy.
