# Security

Talocart is a development beta. Only the current development branch is
maintained; no production support policy or security response SLA is promised.

Report suspected vulnerabilities privately using **Report a vulnerability** in
the GitHub repository's Security tab, when enabled. If private reporting is not
available, ask the maintainer for a private contact without posting exploit
details in a public issue. Never send credentials, cookies, personal customer
data or database dumps in a public report.

Keep PostgreSQL and the application listener private. Terminate public HTTPS at
a local reverse proxy and configure the exact `server.public_origin` together
with secure cookies. Restrict access to the operator until initial setup creates
the owner. Never expose storage or secret directories directly.

Native packages execute trusted code with application access; a separate process
is not a sandbox. New native package archives are keyless: their hashes detect
corruption, but do not authenticate a supplier. Obtain executables from trusted
sources and verify their provenance and compatibility before installation.
Application-distribution and privileged-updater signatures remain separate and
required. Backups contain secrets and personal data:
restrict access, encrypt external copies and test recovery.

Review [operations](docs/release-operations.md), [package security](docs/installable-packages.md)
and the [production checklist](docs/operational-readiness-checklist.md).
