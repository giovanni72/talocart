# Development procedures

Read the local diff, [plan](../IMPLEMENTATION_PLAN.md) and
[architecture](architecture.md) before changing behavior. Keep work scoped and
preserve unrelated edits. Use the [test policy](testing-policy.md) before running checks.

## Rust, data and security

Keep workspace lints and `unsafe_code = "forbid"`. Application input/I/O failures
return `Result` with explicit causes; no panic/unwrap/expect. Test assertions are
allowed. Bound blocking operations and external I/O; never hold synchronous
locks across await. New dependencies need a concrete need, license/maintenance
review, compatible toolchain and updated lockfile.

Use parameterized values, allowlisted SQL identifiers, pagination and database
constraints. Money is integer minor units with explicit rounding and checked
overflow. Server prices, stock and permissions are authoritative. Keep provider
calls outside transactions and order/payment actions idempotent.

Migrations distributed to users are immutable. Add preserving upgrades and test
clean install plus supported previous versions. Do not infer authorization to
reset data or repair all checksums. Doctor/serve do not migrate. Development
databases with divergent histories require an explicit preserving conversion.

Sessions, revocation, ownership, permission and CSRF checks also apply to HTMX.
Preserve escaping/sanitization, private diagnostics and loopback binding. Never
log secrets, SQL text, access-bearing URLs or unnecessary personal data.

## Interface and documentation

Preserve the existing Askama/HTMX/CSS stack. Reuse shared components in
`assets/ui.css` and `crates/web/templates/ui.html`; inspect `/admin/components`.
Labels, associated errors, keyboard, focus, mobile and native fallback are part
of the contract. Include English and Italian together in shared catalogs for
every user-facing change. Preserve user/imported content and explicit translations.

Repository documentation uses English. Update the owning guide for changed
contracts; README stays introductory, the plan holds open work and machine test
results stay in runner/CI reports. Record decisions, visual review and nonreproducible
limits once, linking the machine reports rather than copying them.

## Extensions, releases and source publication

Provider/importer implementations stay outside the base. Preserve generic SDK,
protocol and database contracts and test real consumers. App version, SDK SemVer,
protocol and database contract are independent; a version bump does not require
all other contracts to change.

Use `scripts/release/version.py prepare X.Y.Z` and `check`. Build official
packages from a clean final main commit with the matching annotated tag. Never
move published tags. Packaging, approval and deployment are separate actions.

The public source file list is explicit in `scripts/release/base-files.json`.
Review every new entry; unknown working files are not automatically exported.
Use `python3 scripts/release/source.py --check .` in a tracked public checkout.
For the first initialization, private preparation exports a history-free tree;
initialize the new repository there without copying development Git metadata.
After initialization, update the existing base checkout with ordinary new commits,
preserving its `.git` and ancestry. Once published publicly, preserve the published
history and tags: do not reinitialize the repository or force-push source snapshots.

## Review details

Keep lint exceptions narrow and justified. Document shared API invariants; avoid
unnecessary copies, shared state or artificial lifetime/generic abstractions.
Review dependency advisories/features and pinned-toolchain compatibility without
updating unrelated dependencies. Check indexes and query counts for new data access.

Identity/bootstrap changes must cover concurrent one-use setup, failed login and
rate limits, session rotation/expiry/revocation, logout, expired/replayed recovery
tokens, CSRF, permission denial, restart persistence and HTTPS cookie attributes.
Full pages and HTMX fragments share validation and authorization; distinguish
cache policy explicitly. Use maintained password-hashing/randomness libraries.

Shared UI variants belong in the existing components and component examples;
page templates own only page-specific content/layout. Use `ui-button`, `ui-control`,
`ui-check`, `ui-message` and `ui-empty` with existing macros where applicable.
Preserve the selected language, URL contract and user-owned translations.

For incompatible shared SQL changes, update the database contract independently
of SDK/protocol/application versions. Preserve migration prefixes for compatible
extensions and test rejection of incompatible checksums/ledgers. Do not replace
a complete host checkpoint with a digest from potentially inconsistent state.

## Diagnostic language and JSON events

CLI help, operational output and technical log messages use English. HTTP error
presentation maps typed errors to the EN/IT catalogs; do not translate user content
or replace localized UI errors with technical `Display` strings.

Reserve the tracing `message` field for the event summary. Attach sanitized causes
under `error`; never define `message` explicitly together with a formatted summary.
JSON consumers must receive unique field names without losing the cause.

## Vendored build dependency

The workspace patches `askama_derive` 0.16.1 with the minimal upstream TOML-parser
replacement documented in [the patch record](../vendor/askama_derive/TALOCART-PATCH.md).
This removes `basic-toml` without suppressing its advisory or disabling shared
template configuration. Keep upstream source and licenses together; the base
export includes the patch and adjusts its workspace-relative path. Release
notices include vendored dependencies as well as registry dependencies.
