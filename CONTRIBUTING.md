# Contributing

Read [AGENTS.md](AGENTS.md), the [plan](IMPLEMENTATION_PLAN.md),
[architecture](docs/architecture.md) and [development procedures](docs/development.md).
Keep changes focused and explain the problem, resulting behavior and validation.

Documentation, README files and contribution discussions should use English.
User-facing changes must preserve both English and Italian in the shared
translation catalogs; preserve user/imported content in its original language.

Add a failing regression before fixing a bug, then verify it passes with the
fix. Choose checks with the [test policy](docs/testing-policy.md) and
[commands](docs/guides/testing.md). Use dedicated PostgreSQL/media resources and
simulated providers. A documentation-only change does not require application
builds or database suites. Report only checks actually run and disclose limits.

Do not commit credentials, private configuration, customer data, media uploads,
database dumps or logs. Report vulnerabilities through the [private security
procedure](SECURITY.md). Keep dependencies locked, preserve third-party notices
and justify new dependencies and abstractions.

This repository owns the base and generic extension interfaces. Implementations
of separately distributed providers/importers belong in their own repositories.
Source exports use a reviewed file allowlist; add new public files to
`scripts/release/base-files.json` and run its validation when changing the file set.

Contributions are provided under the project's MIT license. Git publication,
remote CI, human review and deployment are separate milestones.
