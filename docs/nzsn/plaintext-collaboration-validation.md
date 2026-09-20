# Plaintext collaboration: implementation and validation

Status: implementation complete; live qualification and focused regression checks passed. Three broader-suite failures remain unresolved; this is not a fully green release gate.

This record accompanies [the design](plaintext-collaboration-design.md). The default remains encrypted mode. The installed user CLI has not been replaced.

Sanitized live evidence is recorded in [plaintext-collaboration-live-evidence.json](plaintext-collaboration-live-evidence.json).

## Backend contract qualification

The following live API probes ran on 2026-09-18 using existing credentials, with synthetic task text only. Credentials and raw ciphertext are not included in this record.

| Probe | Model | Result |
| --- | --- | --- |
| Unencrypted `collaboration.spawn_agent` schema | `gpt-6-astra` | HTTP 400: the tool name is reserved and must match its configured schema |
| Encrypted `collaboration.spawn_agent` control | `gpt-6-astra` | HTTP 200; opaque message argument |
| Unencrypted `collaboration_plaintext.spawn_agent` schema | `gpt-6-astra` | HTTP 200; exact readable task text; `encrypted_function_args` absent |
| Attributed ordinary user input requesting `PONG` | `deepseek-flash` | Completed with exact `PONG` |
| Subsequent attributed user input requesting `PONG2` | `deepseek-flash` | Completed with exact `PONG2` |

The alternate namespace is therefore required for the tested OpenAI model. Plaintext mode advertises one effective tool family: the configured default `collaboration` becomes `collaboration_plaintext`; nonreserved configured namespaces are preserved. These direct API probes establish the individual provider contracts, not execution through the patched CLI.

## Agent ownership and integration

Four `general-purpose-gpt` agents implemented bounded portions, with parent review and remote validation:

1. **Configuration and persistence:** typed mode, backward-compatible session metadata, local/in-memory storage, resume restoration/conflicts, and cold-reload locking.
2. **Tool schemas and argument handling:** effective namespace, encryption marker policy, direct/code-mode classification, explicit ciphertext rejection, and tracing/telemetry redaction.
3. **Recipient delivery and context:** provider capability, deterministic request projection, attribution/authority instructions, fork admission, byte bounds, and bounded completion messages.
4. **Integration tests:** provider matrix, advertised schemas, actual outgoing requests, no-side-effect rejection, and queue/follow-up/completion behavior.

Parent review required fixes for multipart boundaries, actual recipient lookup, completion-envelope overflow, preservation of legacy native behavior, and outer code-mode logging. Raw model/rollout/UI payloads remain available under the explicit plaintext mode; telemetry and debug payload logging are redacted.

## Recorded checks

Builds and tests run on Ubuntu WSL2 on `windows-dev`, in `/home/jlc/Codebase/codex-provider-matrix-9acb2d6`. The source baseline is `9acb2d624c1d9070f787749d82b3fd598f7a61cb`. Transfers are verified by SHA-256 manifests. Rust is pinned to 1.95.0; Cargo uses eight jobs and debug symbols are disabled for the validation builds.

| Check | Recorded result |
| --- | --- |
| Configuration schema generation | Passed; new payload enum/setting generated |
| Stable and experimental app-server schema/export generation | Passed |
| Initial shared-crate test batch | 880 passed |
| Focused plaintext tests, iteration 4 | 24 passed |
| Provider delivery matrix, iteration 4 | 12 passed |
| Affected-crate batch, Ubuntu 24 iteration 11 | 7,019 run: 6,985 passed, 32 failed, 2 timed out, 11 skipped |
| Final focused regression batch, iterations 13–14 | All 76 selected checks passed across the two runs; recovered 31 of the 34 broader failures and covered the new completion-envelope regression |
| Serial isolation, iteration 12 | Reproduced three still-unresolved failures; the fourth failing case was a collaboration fixture subsequently corrected and passed |
| Patched CLI build and live direct-tool exchange | Passed; rollout audit confirmed OpenAI parent, DeepSeek child, exact `PONG`/`PONG2`, and two child turns |
| Live cold resume without the mode setting | Passed; existing DeepSeek child returned `PONG3`, with no extra child turn |
| Live conflicting encrypted-mode resume | Rejected before running the supplied task |
| Complete workspace suite | Not run; approval was requested under `AGENTS.md` and has not been received |
| Final scoped Clippy cleanup and repository formatting | `just fix -p codex-core` and `just fmt` passed in iteration 16; no tests rerun afterward |
| Generated artifacts and final whitespace check | Retrieved with verified input/output SHA-256 manifests; `git diff --check` passed |

The focused tests include OpenAI native plaintext input, DeepSeek attributed user input, the subordinate-authority instruction, namespace selection, explicit ciphertext rejection before child allocation, cross-provider fork rejection, byte limits, legacy native behavior, and code-mode redaction. The scheduling test uses the existing `wait_agent` barrier and checks that queue-only messaging does not initiate a child request, followed by an ordered follow-up and returned completion.

The live CLI audit additionally verified the tool sequence `spawn_agent`, `wait_agent`, `send_message`, `followup_task`, `wait_agent`; every call used `collaboration_plaintext`. Both session headers recorded plaintext mode, the child stored three plaintext inter-agent inputs, and the parent received two plaintext completion messages. The first live run reported an unavailable adjacent code-mode host. After provisioning the exact matching host, the cold-resume probe ran without startup errors. The audited child had exactly three replies and three turns across the original exchange and resume.

The first broader test attempt was stopped because required workspace executables had not been built. Its environment failures are not counted as behavioral regressions. Provisioning `codex-code-mode-host` then exposed a baseline V8 artifact issue: the upstream default URL lacks the sandbox archive for the pinned version. The repository's [V8 consumer-artifact guidance](../../third_party/v8/README.md) specifies the exact Codex-published archive and binding pair. Use `scripts.codex_package.v8.resolve_codex_v8_cargo_env` to verify the trusted release manifest and both checksums, setting `RUSTY_V8_ARCHIVE` and `RUSTY_V8_SRC_BINDING_PATH`. The verifier requires Python 3.11 or newer; the validation runner used uv-managed Python 3.12. The host built successfully with the verified pair. No V8 pin or source change is part of this feature.

Native WSL testing also encountered the packaged Ubuntu 24.04 zsh requiring `GLIBC_2.38` on an Ubuntu 22.04 host, and sandbox rejection of WSL mountinfo entries. The final test runner used an official Ubuntu 24.04 base rootfs inside WSL2, a private mount/PID namespace, `pivot_root`, a non-root test account, and `tini` as PID 1. This resolved the libc, mount-layout, and orphan-process-reaping failures without disabling sandbox checks. Tests used a private writable source overlay and the shared target cache; final cleanup wrote to the task-owned checkout. The image checksum was verified against its published SHA-256 manifest. This validates the WSL2 Linux configuration, not native Windows or macOS.

Legacy encrypted collaboration fixtures now identify their mock provider as OpenAI-capable and disable request compression where raw mock matching requires it. Local-compaction fixtures retain their non-OpenAI provider and explicitly select plaintext mode. The durable-role fixture selects an existing parent-configured provider. Five scenario snapshots contain only seven expected prompt-summary updates after removing the hardcoded collaboration namespace example.

The remaining failures were reproduced serially:

| Test | Observed result |
| --- | --- |
| `suite::hooks::async_hook_finishing_while_idle_waits_for_the_next_turn::user_turn` | Timeout waiting for the next turn to complete |
| `suite::skill_approval::shell_zsh_fork_skill_scripts_ignore_declared_permissions` | Timeout waiting for an event |
| `suite::unified_exec_zsh_fork_approvals::unified_exec_zsh_fork_parent_approval_preserves_denied_reads` | Filesystem sandbox helper rejects an unreadable `secret.env` glob while loading instructions |

These tests were not skipped, weakened, or counted as passing. Their relationship to the unchanged baseline has not been established by a baseline rerun, so they remain unresolved rather than being claimed as pre-existing failures.

Final Clippy cleanup grouped the router's collaboration tool identities and payload mode into `CollaborationMessagePolicy`, preserving the values at all seven call sites. The final Clippy run was clean; an unrelated pre-existing unused-import fix in `openai_file_mcp.rs` was excluded from the delivered changes. The generated stable export bundle changes only the internal `RolloutLine` schema for the optional mode field; public TypeScript/JSON exports and the experimental bundle are unchanged.

Validation logs were retrieved to `/tmp/codex-plaintext-results/logs/`. The final source/artifact transfer has SHA-256 `76ca15b7ccc755fb733cb5f49dcd96be2dcf17429131ab4cf6a6a55a1fc9214a`. The temporary live-test credential copy was removed; original credentials and the installed CLI were left untouched.

The earlier 6-pass/6-fail diagnostic matrix was intentionally synthetic and is superseded by the explicit plaintext-mode matrix. Compiler/test-fixture failures during implementation were fixed before recording the passing iteration above.

## Release-tag integration

On 2026-09-20, `rust-v0.155.1` was merged into local `main` as `a13c9ff21f`. All 99 pre-merge modified/untracked files, including this implementation, were restored byte-for-byte. The alternative plaintext commits `3047aef18b` and `368839bd65` were not merged.

The merged TUI passed both targeted reasoning-default tests. After reviewing version-only snapshot updates, its full suite recorded 4,904 passes, five WSL-specific image-paste-shortcut snapshot failures, and seven skips. Native Windows compilation succeeded; the ACL regression test failed opening its temporary sandbox directory. Repository formatting passed. These results do not clear the earlier unresolved feature-validation or manual-review gates.

## Review and landing stages

The complete working-tree change exceeds the repository's normal single-change review limit. It includes mechanical struct-literal updates, generated export bundles, and substantial test fixtures. It should be split before landing; no commits or deployment are implied by this implementation record.

The smallest coherent first stage is the backward-compatible **payload enum and durable metadata plumbing**: protocol/session metadata, local/in-memory recorder paths, preservation through revert, storage tests, and required literal defaults. Keep all production writers on the existing encrypted default in that stage and defer the public plaintext TOML opt-in until the receiving path is complete.

A dependency-ordered review sequence is:

1. Durable data model and persistence, with legacy defaults and disk round-trip coverage.
2. Bounded typed context fragments and deterministic recipient projection, initially dormant for the new user option.
3. Task admission, fork restrictions, target-policy lookup, and bounded completions.
4. Mode-aware tool schemas and effective namespace selection.
5. Direct/code-mode normalization and redaction, including the outer code wrapper.
6. Enable the TOML option and immutable resume/inheritance policy once both sender and recipient paths are present.
7. Review the wire tests and scheduling tests as separate coherent test changes; regenerate and review schema/export artifacts against the final implementation.

This is a proposed split of the actual changed modules, not a claim that each intermediate patch already exists. Keep each extracted logical stage below the normal 500-line target for complex changes and 800-line nonmechanical limit; do not expose a partially working plaintext option in an intermediate landing.

**P0 context-review gate:** the new compatibility fragment is capped at 4,096 UTF-8 bytes but can exceed 1,000 tokens. The repository requires an additional manual review before landing. The fixed developer instruction is small and stable; the larger agent payload retains attribution and is explicitly subordinate to system, developer, and actual user instructions. Automated tests and agent review do not waive that gate.
