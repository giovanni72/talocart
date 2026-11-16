# Test policy

Normative procedure linked from [AGENTS.md](../AGENTS.md). Commands and
measurements are in the [test guide](guides/testing.md).

## Select before executing

Read the diff, plan and recent results. Declare changed contracts, consumers,
selected groups and valid prior evidence. Inspect dedicated database/browser,
dependencies and binaries before expensive runs. Use runner `--list` and
`--dry-run`. An audit is not an instruction to automatically run full builds,
all tests and identical rebuilds.

For bugs, reproduce with a minimal failing regression, fix it, run the same
test green and cover affected consumers. Use the least expensive level that
actually proves the contract: pure unit rules, HTTP authorization/validation,
real PostgreSQL for persistence/transactions/concurrency and browser for actual
interaction/rendering/focus. Do not duplicate the same rule matrix at every level.

| Change | Required local selection |
| --- | --- |
| Documentation only | Check references/commands and `git diff --check`; no artificial application build/suite |
| Scripts, runner or CI | Tool regressions, syntax/configuration, catalog dry-run and real orchestration smoke; build scripts also require an isolated real Cargo smoke |
| Confined Rust rule | Relevant unit/regression tests and callers, workspace fmt and affected-target Clippy |
| Area handler/service/query | Relevant unit and real HTTP/PostgreSQL tests including errors/permissions and applicable concurrency; browser for visible paths |
| Confined schema/business/permission change | Affected PostgreSQL consumers, preserving clean install/upgrade for migrations, relevant unit/Clippy and visible browser flows |
| Shared schema/identity/CSRF/authorization/money/package contract | Map all consumers; full workspace/PostgreSQL when impact is cross-cutting or cannot be bounded; full browser for shared authentication/navigation/UI |
| Page CSS/JS/template/text | Build changed embedded inputs, relevant HTTP/template and browser EN/IT, desktop/mobile, JS/no-JS where applicable, plus visual review |
| Shared theme/component/JavaScript | Browser consumers, full browser matrix for cross-cutting impact |
| Dependency/toolchain | Audit plus consumers; full coverage for toolchain/shared runtime changes or uncertain impact |
| Explicit full suite | fmt, Clippy, workspace, full PostgreSQL, full browser and package groups; applicable release/restore/benchmark checks |

Broaden when a change can affect consumers outside the selected area; explain
why a full suite is needed. File/crate names or the presence of SQL alone do not
justify rerunning everything. Never omit, ignore or weaken a failing required test.

## Production candidates

Classify the cumulative diff from the last production-qualified release, not
only the last commit. Record baseline, candidate, affected contracts/consumers,
groups and exclusions. Include distribution scripts, privileged code, migrations,
dependencies and supported external packages.

- Confined releases use the mandatory minimum plus all impacted groups.
- High-impact changes require `full` plus the mandatory minimum and applicable
  operational drills: identity/session/CSRF, shared authorization/money, order/
  payment/stock transactions, shared schema, package protocols, privileged updater,
  backup/restore, toolchain/shared runtime dependencies or uncertain cumulative impact.
- First production use or a new distribution platform requires `full`, complete
  operational checklist, real restore to new targets, failures, benchmarks, visual
  review and target-environment qualification.
- Documentation-only changes without new binaries use documentation checks.

Every new binary release must:

1. Verify Cargo/lockfile version, annotated tag, clean checkout and provenance.
   Run dependency audits before costly tests, resolve applicable vulnerabilities
   and record maintenance warnings separately.
2. Run fmt, architecture checks, relevant Rust unit/Clippy, regressions and selected
   groups. Check asset bundles, notices and packaging tools. Explain inapplicable checks.
3. Compile actual distribution targets from the tag. Verify final Linux executable
   identity, architecture, manifests, digests and signatures on the actual Linux
   target. Promote the same tested artifacts without rebuilding between environments.
4. Use those exact final artifacts and synthetic resources for clean install and
   supported upgrade: explicit migrate, doctor, start, live/ready, EN/IT, admin access
   and graceful shutdown. Verify coordinated backup and all preserved data/files.
5. Verify supported package compatibility; retain unchanged previous executables
   when testing compatibility rather than silently recompiling them all.
6. Stage the same artifacts with HTTPS, login, essential commerce, changed flows,
   service health and human EN/IT visual review before production.

Backup integrity does not prove restore. A real restore drill is mandatory at
first release and after backup/restore, migrations, storage/package state or
database-tool changes. Updater changes require systemd/fencing/interruption/recovery
drills. First-install qualification includes preview without mutations, signature
tampering/reinstallation refusal and database/TLS/kill/reboot/disk-full failures,
with preserved data and startup blocked while incomplete. Benchmark first release
and relevant query/index/lock/pool/concurrency/critical-path changes.

Keep one release evidence record identifying artifacts, selection, environment,
results, visual review, omissions and limits; link runner reports. `--verified`
is an operator attestation, not automatic enforcement of this policy. Missing,
failed or blocked mandatory checks prevent production approval.

## Build order and evidence

Stabilize with focused cycles, then run one final selected verification. Do not
compile concurrently into the same Cargo target. Normal browser verification
uses `cargo build --locked -p talocart` then the debug binary; the runner does
not build. Rebuild after changed Rust/templates/assets/seeds. Reserve release
builds for distribution, performance and explicit operational qualification.

Debug identity covers executable inputs and Git refs; documents/tests/staging do
not force rebuilding the application. Cargo still recompiles changed test targets.
Use `TALOCART_BUILD_IDENTITY_EXACT=1` when packaging debug artifacts. Release
identity always covers the exact full checkout.

After failure repeat the failed group and consumers invalidated by the fix.
Do not restart already-green suites without relevant changes or concrete doubt.
An old binary's existence/fingerprint does not prove a current build.

Runner results live in `.local/test-runs/`. No implicit reuse: `--resume` only
reuses passed evidence under 24 hours with unchanged input/command/binary/runtime/
configuration fingerprints. Do not use it after environment changes, when validity
is uncertain, to investigate intermittent bugs or for required candidate checks.
Distinguish RUN from REUSE. CI does not use resume.

## Isolation and coverage

Use synthetic data, dedicated databases/schemas/media, separate ports and simulated
providers. Never use shop databases, production credentials or paid services.
PostgreSQL runs serially with `--test-threads=1`: schemas do not isolate advisory
locks. The runner prevents concurrent checkout use. Do not bypass that lock.

Preserve EN/IT, both themes, desktop/mobile, keyboard/focus and JS/no-JS coverage
where applicable. Screenshots are not human approval. Full runner coverage does
not include separate dependency audits, distribution/restore drills and benchmarks.
Remote CI is manual and independent of publication or deployment.

On every production server still perform platform/version/module/disk preflight,
coordinated verified backup, maintenance, migrations, doctor, readiness and HTTPS
smoke. Never run synthetic application suites on production data. Define recurring
recovery drills with owners and evidence separately from individual releases.

## Maintaining the verification system

Do not substitute mocks for transaction, migration, concurrency or security
integration checks. A shared pre-migrated database needs demonstrated isolation,
cleanup and separate upgrade coverage before adoption. Any future parallelism
requires independent databases, ports, storage, mail inboxes and result directories,
explicit concurrency limits and comparable measurements.

Add new groups/tools to the applicable manual CI coverage. Ignored PostgreSQL
cases run explicitly; preserve main/release evidence even when superseded PR
runs are cancelled. Optimize duplicate orchestration before expensive setup,
and use recorded durations rather than unmeasured speedup claims.
