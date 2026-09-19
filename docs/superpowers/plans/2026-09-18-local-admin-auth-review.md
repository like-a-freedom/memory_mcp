# Local administrator authentication — independent source and security re-review

**Date:** 2026-09-18  
**Disposition:** Documentation strengthened; **not implementation- or release-approved**.  
**Reviewed:** [Specification](../specs/2026-09-18-local-admin-auth.md), [implementation plan](2026-09-18-local-admin-auth.md).

## Executive assessment

The prior plan was not safe to execute as written. It combined credible security goals with incomplete persistence assumptions, inconsistent interfaces, misleading test slices and packaging steps that had not been verified. The most important defects were not editorial:

1. The existing SQL abstraction does not return the final result of a multi-statement transaction automatically.
2. Credential-generation checks alone do not serialize logout against an admitted mutation; the presented session must participate in the write conflict.
3. Existing bearer cache hits do not recheck key expiry/revocation, contradicting the proposed revoke/resume guarantee.
4. Existing provisioning discovery can omit `NamespaceCreating` after a crash and includes suspended rows.
5. Existing error mapping, configuration composition and Docker/Compose behavior cannot simply be reused for local auth.

The revised documents explicitly address these issues as **proposed implementation work**, not claims that the application is now fixed. The plan was rewritten around one interface ledger and ten tasks, with complete future test modules and a forced-order security experiment matrix. Unsupported snippets were removed rather than hidden behind a blanket “illustrative” disclaimer.

R1–R12 remain present. CLI-supplied username and all technical policies remain proposals, not newly invented approved requirements. R12's original two-document boundary is recorded; the later explicit request authorizes this third review document, not application work.

## 1. Method and scope

- Investigated live source through graph overview, semantic/structural searches, signature views and targeted file reads. Source bodies were authoritative over summaries/comments.
- Checked exact paths and actual signatures, including visibility, feature gates, real composition roots, CLI exhaustive matches, both durable engine branches and the live OIDC session type.
- Reviewed security by tracing commit boundaries, stale snapshots, response loss, mode changes, key cache behavior and packaging paths. No delegated child audit was available; this report does not claim independent subprocess reviewers.
- Edited only the requested specification and plan, then created this explicitly requested report after verifying the parent directory.
- Preserved the preexisting modified README. Initial/final hash checked: `7735beb96f0a306a33ac02ade60053f187a459d172a556dfdf117fa6287038b9`.
- No application implementation, dependency edits, migration files, generated outputs, commits, deployments, live recovery or memory capture writes were made.

**Evidence labels:** source-verified means observed current code; risk means inference from that code or the previous design; correction means a documentation change/proposed implementation contract. A source finding is not a reproduced production exploit. Remote transaction guarantees, KDF behavior and browser flows remain untested here.

The spec and plan were already untracked at the first recorded Git status, so Git cannot provide their original line-by-line baseline. Review used their supplied/current contents. The README modification predates this work and is not attributed to it.

## 2. Critical/high findings and corrections

Severity refers to executing the prior design without correction. It does not assert a deployed local-admin feature already exists.

### F01 — High: transaction return decoding was incompatible with the planned commands

**Source:** `crates/memory-mcp/src/http/registry/surreal_store.rs:49–52,64–95,113–144` (`SurrealHandle`, both `query_json` implementations).

**Verified:** Statement errors are checked, but result index 0 is decoded. A BEGIN/LET/UPDATE/RETURN transaction is not guaranteed to put its public result there.

**Risk:** Empty/misdecoded results could be interpreted as failed authorization, missing resource or success without proving the transaction's postconditions. The previous partial SQL example did not establish the complete transaction or correct result extraction.

**Correction:** Plan §3.3/T2 adds `query_json_at` for Local and Remote, explicit result indices, all-statement error checks and strict decoders. Old index-0 behavior remains unchanged for existing callers. Tests must discover/verify actual result layout on SurrealDB 3.2.4. Removed the incomplete SQL sample.

**Remaining gate:** Execute Mem and remote transaction/error/rollback tests; source inspection is not a substitute.

### F02 — Critical design gap: recovery fencing did not also fence logout/rotation

**Source:** Existing OIDC resolve/touch in `crates/memory-mcp/src/control/session.rs:72–106`; session persistence in `crates/memory-mcp/src/http/registry/surreal_store.rs:2210–2304`. Proposed local transactions in the reviewed documents had no complete shared-session contention contract.

**Risk:** Middleware could authorize a request before logout; a subsequent admin-row-only mutation could still commit after the session was revoked. Snapshot reads do not prove multi-record serializability.

**Correction:** Spec §7 and plan §3.3/T4 require conditional write guards in consistent order: policy → admin → presented session → client → resource. Check current policy, generation, both session deadlines and recent auth inside the mutation. Logout and rotation write the same presented session guard. Forced tests cover both serialization orders, recovery, expiration while waiting and response reordering.

**Clarification:** A mutation committed before logout/recovery may return afterward. No promise of retroactive cancellation. Old-session logout after rotation cannot revoke an independently issued new session.

**Remaining gate:** Real Surreal MVCC/conditional-write behavior, bounded retries and contention cost.

### F03 — High: contention version was incorrectly treated as credential revision

**Source:** Prior plan's CredentialSnapshot carried `version`, and its login comment required generation/version equality after KDF, while the design also used that version for ordinary guard writes.

**Risk:** Unrelated session touches/client actions could make a correct password fail. Removing the comparison without preserving credential fencing would instead admit stale credentials.

**Correction:** CredentialSnapshot now has state/generation/PHC, not contention version. Session insertion compares those exact credential values under shared guards. A test explicitly distinguishes recovery during KDF (reject) from ordinary touch during KDF (allow).

### F04 — Critical design gap: durable mode policy and OIDC handling were incomplete

**Source:** `crates/memory-mcp/src/http.rs` (`assemble`); `crates/memory-mcp/src/control/application/oidc_signup.rs`; `crates/memory-mcp/src/control/oidc/handlers.rs`; `crates/memory-mcp/src/http/registry/storage.rs:379–422`; live OIDC session at `crates/memory-mcp/src/control/session.rs:29–58`.

**Verified:** Existing OIDC persistence has no proposed mode epoch. General account bundle creation is also used outside OIDC. The live persisted OIDC session uses a String cookie hash; a separate registry DTO with the same name uses a byte-array hash.

**Risk:** Router exclusivity alone does not prevent stale callbacks or mixed replicas from creating accounts/sessions. Making the general bundle method OIDC-only would break unrelated callers. A new local table cannot fence old binaries.

**Correction:** Explicit offline transition/old-replica shutdown, OIDC-only fenced bundle method, policy arguments on existing OIDC request/session persistence, and optional epoch on the **live** OIDC session schema/type. Legacy sessions require fresh login; this is now an explicit compatibility proposal. Clear pending OIDC requests and rotate state/session keys on offline transitions. Local identities remain separate; no polymorphic identity refactor.

**Remaining gate:** Approval of the additive schema/legacy relogin and tests of callback bypass plus away-and-back transition.

### F05 — High: key drift/rotation could bypass shared throttle budgets

**Source:** Proposed HMAC-bucket throttling and local-key configuration in the documents; existing config keys in `crates/memory-mcp/src/http/config/types.rs`.

**Risk:** Replicas with different keys map identical inputs to different counters. Merely retaining old rows during key rotation does not retain effective budgets if bucket placement changes.

**Correction:** Durable policy compares domain-separated session/CSRF key fingerprints; Local requires them and OIDC has none. Joining cannot overwrite them. Offline rotation advances epoch/revokes browser state and delays public auth until the longest old window (15 minutes) expires, retaining rows. No new online rotation tool.

**Remaining gate:** Operator approval of downtime/rotation procedure and restore drill. Numerical policy is not an approved product requirement.

### F06 — High: throttling was optional at the service boundary and proxy policy undefined

**Source:** Previous plan exposed separate `reserve_attempt` and unthrottled login methods. Existing `crates/memory-mcp/src/http/middleware/host_origin.rs:17–55` handles trusted forwarded Host, not forwarded client IP; `crates/memory-mcp/src/http/server.rs:22–25` supplies socket ConnectInfo.

**Risk:** A new adapter could omit admission. Per-route domains could multiply code budgets. Trusting arbitrary forwarding headers permits source spoofing; guessed proxy-chain logic would add unverified scope.

**Correction:** AuthAttemptContext is required by browser credential/code methods; service reserves before lookup/KDF. Login/reauth share one domain, inspect/activate/reset another. Store validates dimensions/ranges; invalid usernames use the same source budget and dummy path. Direct peer IP only, mapped-IPv6 normalization, no XFF/Forwarded/X-Real-IP trust, missing metadata fails closed. Proxy aggregation is explicitly an availability trade-off requiring approval.

**Remaining gate:** Shared-handle/restart/window-boundary/flood/collision tests. Fixed windows are no longer described as rolling-equivalent.

### F07 — High: audit failure semantics could erase events or create unbounded writes

**Source:** `crates/memory-mcp/migrations/045_deletion_and_usage_hardening.surql` existing Account-oriented audit schema; reviewed transaction/audit requirements.

**Verified:** Existing audit's actor/account shape is not a local-admin schema. An event inserted into a transaction that subsequently throws is rolled back.

**Risk:** Claimed failure auditing could be nonexistent. Append-per-denied-request could turn throttling into unlimited durable writes. Raw infrastructure error formatting may leak secrets.

**Correction:** Separate typed local audit schema, atomic success audit, `record_failure` after rollback or committed rejection, deduplication by request UUID for admitted attempts, bounded denied aggregates. Invalid-cookie/stale-mutation/pre-admission failures without reservations use fixed action/reason slots rather than unlimited events. Stale epoch may be recorded as failure data without granting authority. Audit I/O failure503; no successful mutation without its success audit. Privileged DB users can bypass table permissions, so “append-only” is application policy, not tamper-proof storage.

### F08 — High/source correctness gap: cached bearer auth contradicts revoke/resume promises

**Source:** `crates/memory-mcp/src/http/principal/auth.rs:94–175,178–202`; `crates/memory-mcp/src/http/principal/cache.rs`.

**Verified:** Positive-cache path rereads Account but not key expiry/revocation. Miss path and subscription `is_current` read the key. Positive TTL is60 seconds; subscription recheck has its own interval.

**Risk:** Suspend rejects cached principals while Account is suspended, but resume can make an old cached revoked/expired key usable again. A per-process cache flush would not fix other replicas.

**Correction:** T7 explicitly changes fresh-request cache-hit validation in all modes to reread key state/expiry/ownership plus Account. Test warmed caches on both replicas, direct revoke and suspend→revoke/expire→resume. Removed the contradictory acceptance of a60-second key cache revocation allowance. Subscription checks remain default30/max60 seconds; already-admitted work is not cancelled. Data-plane expiry still uses synchronized replica UTC, not a falsely claimed DB clock.

**Remaining gate:** Implementation/regression/performance checks, including exact expiry equality. Application code was not changed here.

### F09 — High/source correctness gap: provisioning crash/suspension behavior was overstated

**Source:** `crates/memory-mcp/src/http/registry/surreal_store.rs:1708–1759,1835–1851`; `crates/memory-mcp/src/http/leases/migration.rs:210–300,470–567`; `crates/memory-mcp/src/http/registry/provisioning.rs`.

**Verified:** Worker discovers Tenant rows, not a queue of provisioning events. Due query omits namespace_creating and includes suspended. Worker stage handling distinguishes fresh Reserved/schema0; blindly reentering after NamespaceCreating/schema0 can encounter schema compatibility problems. Existing operator actions are not atomic Account/Tenant/admin-audit operations.

**Risk:** A client can remain undiscovered after a crash; suspended rows can be processed inappropriately; naive resume could label an incompletely provisioned tenant Ready.

**Correction:** Atomic create retains discoverable Reserved row plus event/history; T7 requires focused due-query/stage/lease fixes and real-worker restart tests. Ready-only local suspension uses coherent Account/Tenant pairs and sidecar `suspended_from_ready`; suspended tenants are not reprovisioned. No invented second queue/runtime.

**Remaining gate:** Reproduction/worker tests against isolated tenant storage. This is source-based risk identification, not a completed runtime bug fix.

### F10 — High: key cap and lifecycle atomicity lacked complete serialization

**Source:** `crates/memory-mcp/src/http/registry/surreal_store.rs:1071–1147,1339–1387`; `crates/memory-mcp/src/control/application/api_keys.rs`; `crates/memory-mcp/src/control/operator.rs`.

**Verified:** Existing bundle is atomic but does not include new sidecar/audit/event. Existing cap uses count+insert transaction; this alone does not establish resistance to phantom/write-skew races across independent issuers. Existing suspend updates Tenant, not the required coherent Account pair.

**Correction:** Shared per-client sidecar write before count; atomic key/verifier/operation/audit, current plan cap and DB-time expiry; separate scoped revoke and desired-state/CAS precedence. Explicit key response-loss behavior never returns a second secret. Distinct-admin cap-at-N−1 tests are mandatory. Suspend/resume updates Account, Tenant version, sidecar version/marker and audit together.

## 3. Interface, execution and packaging findings

| ID / severity | Source evidence / prior problem | Correction |
|---|---|---|
| F11 High | `http/config/types.rs:28–245`, `http/config/parse.rs:114–137`, `http/config.rs`, `http.rs:101–159`: unconditional browser key parsing/OIDC composition; optional plan loader with sibling visibility | Tagged optional browser config, mode before keys, explicit seven local limits, facade exports, no zero-key fallback; real startup/default-plan branching, redacted Debug. |
| F12 High | `http/registry/surreal_store.rs:2009–2022`: ensure_plan is create-if-version-absent, supplied record id; one id cannot represent arbitrary versions | Deterministic distinct `local_plan_v{version}`, unique-version race handling and strict compare of all limits; never overwrite an existing different plan. |
| F13 High | `control/error.rs:24–71`: Validation500, recent auth401, raw Internal eprintln | Separate thiserror LocalAdminError with MemoryError infrastructure, redacted Debug/Display and allowlisted mapper; explicit400/401/403/409/429/503 plus413/415. |
| F14 High | `http/router.rs`: layers attached before later merges; SPA fallback; actual OIDC routes differ from some comments | Protect merged local routes explicitly; strict POST/DELETE Origin even when absent; API/auth404 before fallback; test actual route literals and fixture bypass. |
| F15 Medium | `cli.rs` exhaustive mode_label/into_one_shot; `runner.rs`; previous service required hasher for CLI | Complete enum updates and early dispatch, separate management service sharing authority but no KDF/tenant/model initialization; safe CLI error conversion. |
| F16 Medium | `http/runtime/bootstrap.rs` directly calls assemble; `http/oauth.rs` uses OIDC config; `http/composition.rs:37–60` holds concrete Arc | Added omitted real callers; one concrete store shared through capabilities, no downcast/second RocksDB open. |
| F17 Medium | Previous plan's private module/public integration-test contracts and service→control::secret reference | Keep contracts/services crate-visible; inline direct-store tests; external targets black-box. NEW lower-level credential_material module, control depends on service primitives, not reverse. |
| F18 Medium | Prior T1 config test used default HTTP fixture and only is_err | First assert fully valid HTTPS local baseline; mutate one field and assert intended error, avoid false-positive negative test. |
| F19 High assurance gap | Prior runnable-looking tests omitted imports/helpers, used fake PHC, undefined ready_client/page/expect; remote test called private interfaces | Three complete future Rust modules with imports and a defined real-Mem/KDF fixture; fake SQL/browser/remote slices removed; race tests explicitly acceptance tasks with prerequisites. |
| F20 Medium | Prior key metadata API/service/UI returned Vec while active-key cap does not bound historical revoked keys | Page<ApiKeyMeta> consistently in store, service, API and UI; stable cursor50/max100. Client create consistently202 ClientView/current-view replay. |
| F21 Medium | Prior key helper accepted replica timestamp/deadline before waiting; expiry UI contract required explicit choice | Key material helper creates only id/verifier/full credential; insertion computes timestamps and checked expiry in DB, returns persisted metadata. |
| F22 High release gap | Dockerfile builds only HTTP/control-plane, no UI/CLI; build.rs:29–79 requires real absolute nonsymlink dist; static_assets CSP not browser-boot proof | Source-built UI+both binaries, retain nonroot/no-shell/entrypoint/libs/migration path; actual TLS JS/WASM/CSP/MIME smoke, not index-only success. |
| F23 High configuration gap | docker-compose.yml injects nonempty OIDC keys/example secrets and HTTP defaults | Mode-specific local/OIDC/off environments; omit contradictory keys, require supplied secrets/HTTPS/plan, no production example DB credentials; config --quiet to avoid disclosure. |
| F24 High verification gap | No dx executable; no established browser runner manifest in inspected scripts; prior concrete bundle flag and page/expect assumed tooling | Explicit tool/version/install and verified bundle-layout blocker; NEW harness contracts fail on missing prerequisites. No speculative executable bundle command. |
| F25 Medium | `tests/common/http_server.rs:203–249` injects OIDC/HTTP/bootstrap; discovery mock seeds sessions | Explicit local fixture environment, ConnectInfo, no auto-CSRF in negative helpers; trusted TLS browser separate; callback coverage must be added, not inferred. |

Paths abbreviated in the table are under `crates/memory-mcp/src/` unless otherwise specified. The plan's source inventory expands existing paths completely. These corrections avoid adding email/MFA/admin-role hierarchy, a new capability-registry framework, online mode switching, new MCP tools, a new provisioning queue or a speculative forwarded-IP parser.

## 4. Security invariants now explicit

- Code possession permits username disclosure, not account creation or automatic session issuance; consumption is one-use, current generation/epoch and DB expiry.
- Recovery immediately changes durable generation; only final recovery code remains usable, and an earlier successful response can already be stale.
- Session touch never upserts expired/revoked sessions; reauth preserves absolute lifetime and does not require already-recent auth; logout does not require recent auth.
- Commit-time recent-auth checking is distinct from middleware admission. Shared session guard is distinct from admin credential guard.
- Preauth signatures bind epoch; stateless timestamp checks use server UTC with reviewed skew allowance, while durable session/auth expiry uses DB time. These clocks are not conflated.
- Cookies have secure host scope; duplicate values rejected; reauth Max-Age uses remaining absolute lifetime; both CSRF flows use X-CSRF-Token plus exact Origin.
- CLI no password/code arguments, no tenant/model requirements; one-time stdout is deliberately secret and must not enter CI artifacts.
- Admin cookie cannot call MCP; client keys cannot administer; OIDC cookies do not become admins. Client access requires explicitly issued/audited data key.
- Failure audit does not live in a rolled-back success transaction and is bounded even for invalid sessions; storage errors fail closed without raw logs.
- Plan selection, namespace, ids, credential verifier and client readiness are server-owned. No browser quota/namespace overrides.
- Secret issuance replay returns a public key id/error, not a recreated or stored plaintext key.
- Suspended state is Account=Suspended **and** Tenant=Suspended with provenance marker, not Active/Suspended. Desired-state no-op precedence is specified.
- Fresh bearer auth rejects revoked/expired keys after cache warm-up/resume; subscription polling and already-admitted work have separate documented bounds.
- Restoring a registry snapshot can resurrect generations/credentials: rotate browser keys/invalidate browser state before traffic. Privileged database access remains a trust boundary.

## 5. Checks performed during documentation review

### Passed / observed

1. Live graph and structural/source investigation, including renewed direct checks of the live OIDC session and both persistence signatures after compaction.
2. Explicit parent-directory verification before creating this review file.
3. Python existence checks of **51 existing source-inventory path references** in the rewritten plan: all present. NEW paths checked separately and remain marked proposed/absent; migration047 remains absent. Colon line locators were not treated as literal filenames in the final source inventory check.
4. R1–R12 rows found in both spec and plan; even Markdown fence counts; final newline/trailing-whitespace checks (intentional two-space Markdown line breaks allowed).
5. All **three Rust test code slices parsed successfully** with `rustfmt --edition 2024 --emit stdout`, provided via stdin; no source files generated. This checks Rust syntax only, **not name resolution, type checking, execution or the planned KDF dependency**.
6. Tool inventory: cargo, rustc, node and docker executable paths present; dx absent. Presence does not prove Docker daemon/browser availability or compatible versions.
7. README SHA256 unchanged from the initial recorded value above. Git status still shows its preexisting modification; documentation additions remain untracked. No commit/branch created.
8. Final documentation path/link/table/fence checks and `git diff --check` were run before handoff. Git diff alone does not cover untracked documents; independent checks cover these deliverables.

### Not run / not claimed

- No cargo build/check/test/clippy suite. The authorized change is documentation only, and application interfaces/tests described here do not yet exist. Mandatory project clippy command is retained as an implementation shipping gate, not falsely reported passed.
- No Argon2 compilation/benchmark or memory/cancellation experiment; no approved dependency/version yet.
- No migration application, live store reads/writes for the new feature, remote Surreal concurrency test, fault injection, multi-process session/recovery or provisioning restart run.
- No Docker build/run, Dioxus bundle, wasm compile, browser package install, trusted-TLS browser test or CSP execution test.
- No source code security fixes. Existing cache/provisioning observations remain application work in T7.
- No delegated independent child audit. Parent reviewer should independently inspect this report and updated contracts.

## 6. Remaining approval and runtime blockers

| Gate | What must be provided before implementation/release |
|---|---|
| Technical design approval | CLI username allocation; ready-only suspension/local-only listing; timing/throttle collision/direct-peer trade-offs; key idempotency/one-time-response loss policy; offline mode/rotation and15-minute hold; legacy OIDC-session relogin; CLI/UI image scope. |
| Dependency approval | Exact vetted Argon2 version/features/RNG APIs/lockfile changes; feature-gating and native compatibility. Any browser runner/crypto binding additions reviewed separately. |
| Migration approval | Full eight-table schema plus optional live OIDC-session epoch, constraints/permissions/postconditions/indices, numbering, fresh/upgrade/restart/concurrent apply/rollback procedure. Approval of a design is not approval to write historical/generated migrations. |
| Surreal behavior | Explicit result-index/error semantics and shared write-guard serialization on3.2.4 remote, phantom/cap races, session/logout/recovery/mode ordering, conditional timestamps, rollback audit and retry limits. |
| Operational load | Two-running/eight-waiting KDF behavior and hardware cost; shared policy/admin guard throughput; proxy-source availability and DB-denial aggregation load. |
| Runtime regressions | Cache expiry/revoke/resume all modes; real worker NamespaceCreating restart/suspension; existing off/OIDC/stdin behavior and tenant isolation. |
| Packaging/tooling | Pin/test Dioxus0.7 CLI, exact bundle flags/layout, wasm target; approve/lock browser runner; complete fixture/harness commands; real source-built nonroot image and TLS/CSP boot. |
| Rollout/restore | Stop old replicas, protect privileged CLI/DB access, clear old OIDC state/rotate keys, restore drill and rollback with local control disabled. |

**Handoff recommendation:** Review the revised ledger and security experiment matrix first. If approved, implement task-by-task with the named positive-count RED/GREEN checks and forced race fixtures. Do not interpret this report as authorization or evidence that the planned authentication feature is already secure or runnable.
