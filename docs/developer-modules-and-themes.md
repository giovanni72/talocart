# Developing modules and themes

Read [architecture](architecture.md), [development](development.md),
[module contracts](module-contract.md) and [test policy](testing-policy.md).
Keep extensions independent of base implementation ownership.

## Native extensions

Use `talocart_module_sdk::{start, serve, authenticate, Hooks}` for the shared
process runtime. Keep database/business initialization and the stdin/worker
shutdown signal in the module. The source crate version follows this workspace;
the native SDK compatibility version remains independently declared in platform.

An independently built module implements the host's generic native package
protocol. `crates/package-fixture` contains synthetic contract examples; it is
not an external provider/importer. Use the SDK/protocol/database compatibility
requirements in [installable packages](installable-packages.md).

Declare owner/license, target, routes/capabilities, required database prefix and
private ledger. Keep provider/source clients, credentials, UI/assets and tests in
the module repository. Access commercial data through authorized owner services;
do not introduce a host runtime dependency on an optional module or write commerce
tables from an adapter. No provider network call inside a SQL transaction.

Module pages use typed view models, escaping/sanitization and shared UI with EN/IT
catalogs. Forwarded rendering context never authorizes a mutation: enforce current
identity, permissions, ownership and CSRF. Persist private settings safely; never
return secrets in HTML or logs. Bound process/RPC I/O, blocking work and retries.

Test keyless packaging, descriptor mismatch and payload corruption, compatibility against an unchanged prior executable,
permissions/CSRF, lifecycle interruption, private migrations, concurrency and
preservation on base upgrades. Use synthetic services/resources; external network
tests must be explicit opt-in in the module's own workflow.

### Implementing a new native module

1. Build a separate executable with the current SDK. Use one stable ID and keep
   administrative routes and assets under `/admin/extensions/{id}`. Declare only
   implemented capabilities; the host never branches on a provider/importer name.
   Implement `--describe-package` as a bounded JSON command without database access
   or side effects. It must declare the same identity, routes, permissions, schema
   checkpoint and migrations as the packaged manifest; the host compares these
   before installation or update can run private migrations. Build with
   `talocart package-build --executable /path/to/module --output /path/to/module.talopkg`;
   no publisher key is generated, supplied or registered.
2. Implement the bounded stdin/stdout handshake and authenticated loopback server.
   Validate the host database checkpoint and the exact private ledger.
   Keep installation/migration explicit; serving and read-only checks must not
   migrate. EOF on the host's stdin lifeline must terminate serving and workers.
   Implement SDK 6 `GET /__talocart_package/health`: authenticate the host token,
   return 204 before acquiring request gates, and perform no database/provider I/O.
3. Start required background workers during `Serve`, independently of HTTP visits.
   Monitor their task handles and fail the process on unexpected task completion.
   Persist work and leases before acknowledging it, reclaim expired leases safely,
   and use idempotent writes. The host supervises processes, not arbitrary internal
   tasks; a live HTTP process with a dead worker would otherwise look healthy.
4. Implement quiesce/resume and read-only `CanStop`. Check durable pending/running
   work, drain in-flight handlers and reject unsafe stop/restart/update operations.
   A refused quiesce must leave the module usable. Do not delete jobs to permit a
   lifecycle transition. Shutdown and operator restart are distinct paths.
   The host admits administrative reads and payment calls concurrently. Other
   administrative methods own the host gate exclusively, including against payment
   leases retained through SQL commit.
   Hold your own reader/writer gate for each whole handler: reads share it, while
   mutations and `/__talocart_package/control` take it exclusively, so quiesce
   drains in-flight handlers and a settings change never overlaps work that uses
   the old settings.
5. Compose `AppState::standalone_module`, the shared router and `AdminShell` behind
   the module authentication middleware. The current SDK consumes the optional host menu
   context automatically. Use the shared admin template and native namespace links;
   do not maintain a second host menu or treat context/header fields as credentials.
   Load current sessions and recheck operation permissions/CSRF in owner services.
6. Keep errors actionable without exposing source URLs, SQL, tokens or provider
   payloads. The host reports package/version and sanitized startup/exit/communication
   causes; modules retain responsibility for private domain diagnostics. Never rely
   on raw child stderr being forwarded to the host journal.
7. Verify the packaged executable on a clean installation and a preserving upgrade:
   enabled/disabled startup, restart without an HTTP visit, abrupt child exit,
   nonresponsive live processes and same-process recovery, repeated failures with slow retries and explicit recovery, pending-work refusal, interrupted
   migrations, current/revoked permissions, EN/IT, keyboard and mobile/no-JS flows.
   Keep an unchanged older executable for host-compatibility tests. Rebuilding the
   old source against the new SDK is a different test.

The [process contract](installable-packages.md#process-and-payment-capabilities)
defines retry timing, readiness, diagnostics and navigation wire limits. SDK 6
requires rebuilding modules against the health contract; SDK 5 executables cannot
run. This does not change the handshake, manifest format or database contract.

## Compiled themes

`themes/default` and `themes/essential` share services and form bodies. Theme
choice is persisted from an allowlist, not an arbitrary path. Palette/fonts/density
and responsive image sizing are presentation; they do not change business rules.
Doctor/serve reject a persisted theme absent from the build rather than replacing it.

To add a compiled theme:

1. Define layouts/assets, reusing existing bodies/components and explicit inheritance.
2. Register its ID and selection in `storefront/layout.html`, header/footer selectors,
   shared card sizing and embedded asset routes. No queries in templates.
3. Add a preserving migration for the persistable theme allowlist, with EN/IT labels.
4. Test public pages, errors/fragments, stale/concurrent settings, cache, restart,
   keyboard/focus, both languages, desktop/mobile and JS/no-JS; inspect visually.
5. Rebuild embedded inputs with the pinned toolchain. Merchants run the executable
   without template compilers or Node.

Stale HTMX presentation policy triggers full-page navigation preserving path and
filters; minicart fallback goes to the full cart. Shared commercial/security
checks remain authoritative. Conditional GET/HEAD must preserve asset/cache policy.

`scripts/fixtures/default-theme-reference.css` is a test-only baseline, never a
server asset. Browser `presentation.verify` explicitly compares styles/geometries;
`presentation.screenshot` alone does not invoke that comparison. Preserve existing
verification points and responsive image-size/layout checks across breakpoints.

Select checks by impact rather than copying full suites for every theme edit.
The current PostgreSQL integration targets are listed by the [runner](guides/testing.md).
