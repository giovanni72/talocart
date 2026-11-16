# Changelog

Application releases are separate from SDK, payment RPC and database contracts.
15.0.0 is the first supported base release and database schema.

## 24.10.0

This base snapshot includes the following changes since 18.0.0. Optional payment
provider and import adapter implementations remain separately distributed.

### Breaking
- Adopt native SDK 6 and database contract 4. Native modules must implement the
  authenticated health endpoint and be rebuilt against the current SDK; SDK 5
  executables are incompatible.
- Revise the development schema baseline for stock control and administrative
  order revisions. A fresh installation is required; older baseline checksums
  are incompatible and must not be rewritten to simulate an upgrade.
- Build new native modules as keyless `TALOPKG2` packages. Older hosts cannot read
  the new envelope; compatible legacy envelopes remain readable by this host.

### Added
- Create administrative order drafts and publish audited commercial revisions
  with immutable historical snapshots and preserving stock adjustments.
- Configure guest checkout, select saved/default customer addresses in checkout,
  and control stock management independently of recorded product quantities.
- Manage related products and use an interactive storefront product gallery.
- Share the native module runtime through `talocart-module-sdk`, supervise
  process health and configure required-module readiness and installation mode.
- Display the application version and build identity in the storefront footer.

### Fixed
- Delay checkout contact validation until the customer submits the form;
  initial profile prefill and saved-address selection do not show premature errors.
- Preserve safe rich-text tables and upgrade the HTML sanitizer dependency.
- Improve storefront image responses, conditional requests and gallery behavior.

## 18.0.0 — 2026-11-05

### Breaking
- Remove `seed-demo --expand` and the demo expansion API. Demo installation
  accepts only the current bundled receipt version and preserves existing edits;
  other receipt versions are rejected without conversion or data changes.

## 17.0.0 — 2026-11-05

### Breaking
- Accept only explicit format-2 updater configuration. Remove format-1 deployment,
  publication commands and the `bootstrap_updates` conversion option. Distribute
  updater code only in separately signed administrative/engine bundles.
- Remove the obsolete product bulk-delete route and single-action service API;
  current compound forms and single-product deletion retain their behavior.
- Remove the web extension's Italian-source `message` translation adapter. Use
  Fluent IDs for system labels and typed error presentation at the HTTP boundary.

### Changed
- Remove historical order projection backfills and startup email retirement;
  current checkout writes projections atomically and online confirmations remain
  queued only after verified payment. Keep projection integrity checks.
- Preserve user content, stored diagnostics and published SQL definitions.
- Keep SDK 5.0.0, native protocol 3, payment RPC 2 and database contract 3 unchanged.

## 16.0.0 — 2026-11-05

### Breaking
- Require SDK 5.0.0 and strictly namespace all package administrative routes
  under `/admin/extensions/{id}`. Remove legacy importer route and timeout
  exceptions. Older native executables must be rebuilt and signed; remove
  packages with legacy route manifests using the previous host before upgrading.
- Keep database contract 3 and existing migrations unchanged.

### Changed
- Declare external test binaries in catalogs, sharing prerequisite checks and
  evidence fingerprints without module-specific runner branches.
- Simplify base-only packaging while retaining binary composition validation.
- Use neutral demo catalog names and remove retired provider selectors from the
  visual reference fixture. Existing store data is not rewritten.
- Make public documentation self-contained and keep generated build paths out
  of source control.

## 15.0.3 — 2026-11-05

### Fixed
- Remove the unmaintained `basic-toml` build dependency by backporting Askama's
  configuration-parser replacement with maintained `toml`. Preserve shared
  template configuration, upstream licenses and standalone base builds.
- Include vendored dependency licenses in distribution notices.

## 15.0.2 — 2026-11-05

### Fixed
- Preserve unique JSON log fields, with the event summary in `message` and the
  sanitized cause in `error`.

### Changed
- Use English CLI help, operational output and technical diagnostics while
  preserving localized English/Italian web errors.
- Remove development preview notices from the administrator login, dashboard
  and sidebar.
- Start supported release history at 15.0.0, remove the obsolete update bridge
  and rename active browser tests and the theme reference fixture descriptively.

## 15.0.1 — 2026-11-05

### Documentation
- Consolidate shared English documentation in the base source tree, exported
  unchanged to the standalone repository. Full documentation contains only
  workspace-specific supplements and links to the shared rules.
- Preserve operational, recovery and verification details in their owning guides
  and remove obsolete historical migration instructions and duplicate procedures.

## 15.0.0 — 2026-11-05

### Initial release
- Autonomous modular Rust commerce application with storefront, administration,
  catalog, checkout, orders and offline payments.
- Initial database schema, contract 3; SDK 4.0.0 and native protocol 3.
- Generic extension contracts for separately installed native packages.

### Changed
- Keep the application, built-in modules, tests and documentation in an autonomous
  base tree, with an independently buildable source distribution.
- Separate optional package implementations and integration tooling from base
  sources and tests. Source export copies reviewed application files verbatim.
- Consolidate current documentation and remove obsolete development reports.
