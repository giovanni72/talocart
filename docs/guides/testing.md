# Test commands and measurements

[Index](../README.md). Follow the normative [test policy](../testing-policy.md)
before choosing or running checks. This guide does not claim any current pass result.

## Catalog and selection

`scripts/test-suite.json` is the single catalog. Groups carry commands and
prerequisites; the runner does not compile the application.
Additional executables are declared with a group's optional `binaries` list.
Each entry has an `env` override and a `default` path relative to the runner's
working directory. The same resolved paths are checked before execution and
included in evidence fingerprints. `--dry-run` prints the resolved prerequisites.
Group-specific environment values take precedence over ambient overrides.

```sh
python3 scripts/test-run.py --list
python3 scripts/test-run.py --only rich-text --only postgres-pages --dry-run
python3 scripts/test-run.py --suite full --dry-run
```

Suites are `rust`, `tooling`, `postgres`, `browser`, `packages`, `full`.
Targeted `unit-*`, `clippy-*`, `postgres-*` groups are alternatives to their
aggregates: do not select both. PostgreSQL integration is split among targets
`identity_catalog`, `identity_commerce` and `identity_platform`. The public
catalog omits external module groups; generic package fixtures remain included.

`storefront-images` runs the focused EN/IT, default/essential, desktop/mobile and
JS/no-JS matrix for direct image responses/ETags, compact brand logos, photo
selection/history, preserved purchase forms and the footer build identity.

## Dedicated environment

Require Docker Compose, Node.js 22+, npm dependencies and a browser:

```sh
npm ci --ignore-scripts --no-audit --no-fund
npx playwright install chromium
python3 scripts/test-env.py up
python3 scripts/test-env.py status
```

The `talocart-tests` Compose project uses PostgreSQL `127.0.0.1:55432`, Mailpit
HTTP `127.0.0.1:18025` and SMTP `127.0.0.1:11025`. Random private credentials are
in `.local/test-services.json`; do not delete that file while retaining its volume.
`run` starts services and supplies test configuration, removing application overrides.
Use the installed Chromium, or select an available channel with `TALOCART_BROWSER_CHANNEL`.

PostgreSQL and runner execution remain serial. Schemas do not isolate database
advisory locks. Never run independent runners against the same checkout/database.

## Build before execution

```sh
cargo build --locked -p talocart
python3 scripts/test-run.py --only fmt --only unit-web --only clippy-web
python3 scripts/test-env.py run -- python3 scripts/test-run.py --only rich-text --only postgres-pages
```

The default binary is `target/debug/talocart`, overridable with
`TALOCART_TEST_BINARY`. Rebuild changed embedded inputs before HTTP/browser checks.
Do not substitute an existing executable for evidence of a current build.
For full PostgreSQL coverage when justified:

```sh
python3 scripts/test-env.py run -- python3 scripts/test-run.py --suite postgres
```

Reports/logs/timings are in `.local/test-runs/`. The runner fails fast by default;
`--keep-going` collects independent failures but still exits unsuccessfully.
Review logs before sharing. `--resume` requires valid passed fingerprints and
is unsuitable for uncertain environments, intermittent bugs and candidate qualification.

```sh
python3 scripts/test-env.py stop
```

This stops dedicated services while preserving volumes. Do not indiscriminately
remove volumes to clean up a test environment.

## Tooling, security and distribution checks

```sh
python3 scripts/check-architecture.py
python3 scripts/release/version.py check
python3 scripts/release/source.py --check .
python3 -m unittest discover -s tests/release -v
node scripts/build-editor.cjs --check
```

The source check requires a tracked checkout whose file set matches its reviewed
allowlist; update it for intentional new public files. A fresh snapshot can be
checked with `--no-history` before initializing Git.
The snapshot allowlist must map each path to itself, and linked files are
rejected. Tracked checkout checks support Git worktrees and ignore generated
build output; `--no-history` rejects both `.git` directories and worktree files.

Dependency audits are separate from `full`: run Cargo audit and npm audit as
described in [updates](application-updates.md). Do not suppress maintenance warnings.
Real install/HTTPS/restore uses `scripts/release/acceptance.cjs` with a dedicated
database role allowed to create disposable databases; see [operations](../release-operations.md).
Generic package/update fixtures do not require a real external provider/importer.

The `runtime-recovery` group accepts `TALOCART_TEST_PREVIOUS_BINARY` for a supported
previous executable. `smtp-recovery` verifies encrypted SMTP backup/restore with
new database/storage and rejection of missing/wrong keys. PostgreSQL 17 clients
are required; Docker tests can set `RELEASE_TEST_PG_CONTAINER=talocart-tests-postgres-1`.
Privileged systemd/first-install tests are explicit disposable Linux checks,
not deployments to real hosts. Remote workflows are manual.

## Generic payment regressions

The `orders::online_generic::` cases in `postgres-orders` verify opaque IDs,
settlement/full refund, snapshot preservation, provider isolation, typed webhook
references with equal IDs in different namespaces and conflicting references.
They use a synthetic gateway and require no external payment module or credentials.
The consolidated initial schema requires explicit provider ownership. Fresh installation and repeated migration are covered by the schema tests.

```sh
python3 scripts/test-env.py run -- cargo test --locked -p talocart-web --test identity_commerce orders::online_generic:: -- --ignored --test-threads=1
```

## Browser synchronization and diagnostics

Preserve both themes, EN/IT, desktop/mobile, focus/keyboard and native fallbacks.
For coupon native submissions, register both POST-response and new-navigation
waits before the action; verify POST and final response status. A 303 plus
`waitForLoadState` may observe the previous document even when URLs match.

```sh
python3 scripts/test-env.py run -- node --test scripts/test-browser-coupons.cjs
python3 scripts/test-env.py run -- cargo test --locked -p talocart-web --test identity_commerce orders::coupons::coupons_browser_bilingual_desktop_mobile_and_no_js -- --ignored --exact --test-threads=1 --nocapture
```

The deterministic helper regression holds the redirect response until explicitly
released, without timing retries. Current green tests do not prove the cause of
the historical timeout described in the plan. Coupon artifacts under
`.local/verification/coupons/browser` retain checkpoint/failure screenshots,
limited navigation/status/form-presence diagnostics and synthetic visible text,
without session cookies or hidden field values. Review before sharing.

## Benchmarks and build storage

Use a current release binary, synthetic dedicated PostgreSQL and serial execution.
Inspect npm scripts for catalog, site, commerce, concurrency, checkout, dashboard
and sales-history measurements. Keep query correctness/invariants distinct from
machine-dependent latency. Do not claim a production SLO from local measurements.

The sales-history DST check reads the actual query from commerce, replaces only
the clock with a parameter and covers 23/25-hour Europe/Rome days, period bounds,
currency/status filters and exact totals. Its large benchmark measures 100,000
and 1,000,000 synthetic orders with real constraints/indexes/triggers and plans.
Reports identify query/binary hashes and environment under `.local/verification/`.

Use `scripts/build-storage.py --help` to inspect build storage management. Stop
builds/tests before cleanup, preserve any active executable outside target and
preview only the intended profile. No automatic cleanup or deletion of individual
objects by age. Ordinary debug keeps line tables; `--profile debug-full` is for
interactive debugging and can consume substantially more space.

### Targeted diagnostics and benchmark comparisons

`postgres-orders` includes
`contention_notice_database_failures_preserve_cause_and_retry_atomically`: an
outbox trigger and nontransactional counter verify retries for `40001`/`40P01`,
the three-attempt limit, permanent causes and rollback without duplicate effects.
`indexing` checks real browser cache transitions and asset GET/HEAD/304 behavior;
`settings` checks EN/IT administration and persistence after restart.

For dashboard comparison preserve the previous release executable outside `target`
and set `TALOCART_TEST_PREVIOUS_BINARY`; both executables need compatible schema
contracts/digests. `TALOCART_TEST_DASHBOARD_OUTPUT` selects results. Dashboard
`--smoke` checks the tool with a small dataset and is not performance evidence.
For paired sales-query comparison use `TALOCART_TEST_SALES_BASELINE_SOURCE` with
previous `crates/commerce/src/overview/sales.rs` and `TALOCART_TEST_SALES_OUTPUT`.
Queries alternate on the same data/time and must return equal results before
latencies/plans are compared. Run benchmarks serially without competing load.
General catalog/site results can be separated with `TALOCART_BENCH_OUTPUT` and
`TALOCART_SITE_BENCH_OUTPUT`.

### Variant price and stock summaries

The `variant-commerce` browser group checks configured-product summaries and
parent-value preservation across EN/IT, both themes, desktop/mobile and JS/no-JS.
It includes native navigation to variants, editable shared purchase rules, empty
and single-active-variant states, range prices, validation and simple-product
fields, draft conversion with explicit republication, and persistent guidance
after save toasts expire. This focused group sets `TALOCART_TEST_NO_SCREENSHOTS=1`
for DOM/layout verification without captures.

### Foundation and storefront regressions

`admin-foundation` runs `scripts/browser-admin-foundation.cjs` for installation,
authentication and administration, including configured-to-simple product
conversion: the product stays private as a draft until explicitly republished
through the editor, across desktop, mobile and no-JavaScript profiles.
Demo profiles reuse
`scripts/browser-demo-storefront.cjs` for storefront configuration and rendering.
`fixtures/default-theme-reference.css` under `scripts/` is a frozen test oracle
for computed styles and geometry; it is never served by the application. Update
it only for an intentional, reviewed visual contract change.

## GitHub runner resources

The manual Rust checks workflow uses Ubuntu 24.04, matching local Linux
verification. Cargo compilation is serial with incremental artifacts disabled
to bound concurrent compiler memory and disk use on private standard runners.
Test coverage and warning enforcement are unchanged. Clippy records elapsed
time and maximum resident memory; failed compiler steps also retain memory,
disk and kernel diagnostics. Exit 143 indicates SIGTERM, but alone does not
establish an out-of-memory condition or a compiler defect.

The private runner also reserves 16 GiB of swap after removing unused Android,
.NET and Haskell SDKs from its disposable VM. A single Clippy process can exceed
the runner's physical RAM; serial compilation alone is insufficient. Setup
requires at least 28 GiB free before allocating swap, preserving build space.
The resource diagnostics distinguish this pressure from lint failures.

## Manual CI workflows

- **Rust checks** (`ci.yml`) verifies source quality: dependency audits, formatting,
  all-target Clippy, Rust tests without the ignored database scenarios, source
  export boundaries, tooling contracts, a debug application build and asset/Node
  checks. The standalone base checkout is itself the independent build; Full
  also builds its exported base in a separate target directory.
- **Operational qualification** (`operational-qualification.yml`) preserves the
  broader workflow: PostgreSQL, complete browser coverage, package lifecycle,
  systemd interruptions, optimized build, HTTPS/install/restore and benchmark
  smoke checks. It is explicitly dispatched for operational qualification.

Both workflows remain manual. Passing Rust checks is source verification, not
production approval; use the test policy for the required operational evidence.
