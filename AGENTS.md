# Development rules

Before editing, read [the plan](IMPLEMENTATION_PLAN.md), the local diff,
[architecture](docs/architecture.md), [development procedures](docs/development.md)
and [test policy](docs/testing-policy.md). Preserve existing work, data and scope.

- Keep the modular monolith: app composes, handlers own HTTP, services enforce
  business rules/authorization, data owners execute parameterized queries and
  templates receive typed data without querying. Add abstractions only for
  concrete boundaries.
- Recoverable failures use `Result` with explicit causes. No panic, unwrap or
  expect on input/I/O in application paths. Preserve workspace lints and
  `unsafe_code = "forbid"`. Bound blocking work; no synchronous lock across await.
- Protect invariants with constraints/transactions. Use integer money, explicit
  rounding and checked overflow. Recheck prices, stock and permissions on the
  server. Orders/payments are idempotent. No external providers inside transactions.
- Authenticate and authorize all routes, including HTMX; protect mutations with
  CSRF and never mutate through GET. Preserve escaping, sanitization, private
  logs, secure sessions and loopback binding. Never publish secrets or unnecessary
  personal data.
- UI changes include EN/IT together, shared catalogs, semantic HTML, labels,
  keyboard/focus, mobile and native fallbacks. Reuse `assets/ui.css`, the macros
  in `crates/web/templates/ui.html` and `/admin/components`. Preserve user content.
- Migrations are explicit and preserve data/files. No reset without authorization.
  Once migrations are distributed, add new migrations; do not rewrite published
  definitions. Test clean installation and supported upgrades. Doctor/serve do
  not migrate.
- Tests use synthetic data and dedicated resources. PostgreSQL remains serial.
  Declare touched contracts and selection, use runner `--list` and `--dry-run`,
  reproduce bugs red/green and verify actual interactions in the browser.
  Build embedded inputs before browser tests. Do not repeat green suites without
  relevant changes; never claim an unexecuted suite passed. Visual review matters.
- Update the owning contract document. README introduces the project; the plan
  contains open work; guides describe current behavior. Keep machine results in
  runner/CI reports and one narrative location for decisions/visual review/limits.
- Application version comes only from `[workspace.package].version`; prepare
  with `python3 scripts/release/version.py prepare X.Y.Z` and run `check`.
  SDK, protocol and database contracts are independent. Tag clean final main
  with an annotated tag; never move published tags. Build packages from that tag.
- Keep optional provider/importer implementations outside the base. Review new
  source-allowlist entries explicitly. Publishing Git, packaging and deployment
  are separate actions.

For documentation-only changes check references/commands and `git diff --check`.
Use the existing frontend stack for UI work. Read any adopted skill first and
verify its origin and compatibility; use Git automation only when requested.
