# Installed application updates

[Index](../README.md). Development, tests/builds and publication are local actions.
GitHub stores public source/packages; workflows are manual. Push/tag/upload does
not deploy. Publishing a candidate, approving it and an owner requesting its
installation are separate steps. Scripts do not implicitly commit, push or tag.

## Trust and components

The ordinary application runs unprivileged. A separate root-owned OS service
verifies Ed25519 manifests/digests and installs only explicitly requested approved
releases. Root-code signing is distinct from application release signing. Obtain
trusted public keys independently, never from unverified downloaded code.

The HTTP backend only authorizes private status/requests with owner/CSRF; it does
not use sudo, network publication or service restarts. Package manager rules own
compatibility. The root launcher owns durable fences/recovery and starts reviewed
signed updater engines in separate processes. It never executes arbitrary shell
commands or downloaded application code as root.

Distribution format 2 describes base, selected existing module replacements and
updater per platform. Base-only releases have an empty module catalog. Format-1 feeds are separate from format-2 data. Unified publication emits only
`distribution.json` and its signature, and rejects attached format-1 feed assets.
Public base source publication does not publish external module implementations.

## First installation: base and updater together

The independently root-signed `talocart-install` archive installs the verified
base and updater scheduling together. The ordinary application archive is a feed
component, not a root-authorized installer. Build it only after qualifying final
Linux artifacts from clean annotated main/tag:

```sh
python3 scripts/release/updater_bundle.py --installation --version 1.0.0 --target x86_64-unknown-linux-gnu --artifacts .local/release/artifacts --public-key /private/update-public.pem --root-public-key /private/updater-root-public.pem --key /private/updater-root-private.pem --output .local/release/first-install --verified
```

The version above identifies the launcher, independently of the Cargo application
version. `--verified` is an operator assertion, not automatic production approval.
Publish the installer and detached signature separately alongside the feed assets.
It contains no store data or credentials.

Prepare Ubuntu 24.04, PostgreSQL 17 role/empty database, application account/group,
Python 3.11+, OpenSSL, systemd, Nginx/TLS, complete private application TOML and
trusted root-owned keys. Private repository access, when applicable, uses a
root-private read-only token; public source does not require publishing any token.
Keep media/packages under the service writable root and operator-only access
until setup creates the owner. Do not preinstall the application service for this
first-install path. Domain, certificates and credentials are host-specific.

Verify detached signature **before extraction**:

```sh
openssl pkeyutl -verify -rawin -pubin -inkey /root/trusted-updater-root-public.pem -in talocart-install-X.Y.Z-TARGET.tar.gz -sigfile talocart-install-X.Y.Z-TARGET.tar.gz.sig
```

Extract to root-owned non-writable ancestors. Stage `update-unified.toml` as
0600 with actual paths, complete writer inventory and `feed_format=2`. Only
assert `writers_fenced=true` after implementing those fences. Run preview first:

```sh
/usr/bin/python3 -I /root/talocart-install/install_server.py --config-source /root/update-unified.toml --root-public-key /root/trusted-updater-root-public.pem --nginx-site /etc/nginx/sites-available/shop
```

Preview does not mutate persistent installation state or stop/start the app.
After reviewing it, add `--apply`. Apply refuses existing services/installations
and nonempty databases, verifies identities/signatures, migrates as the application
user, starts/checks base and bootstraps updater timer/path. Success requires both.
Complete `/setup`, EN/IT checks and owner login before opening public access.

An independent systemd installation guard binds app lifetime to the installer
lease until durable success. Kill, unreadable/failed journal or reboot while
incomplete prevents startup and stops the app, even when disk-full/read-only state
prevents writing logs/markers. Preserve partial data/journal for administrative
recovery; never rerun over a partial installation, reset data or edit success
markers to force startup. First-install is not the upgrade procedure.

## Provisioning the updater for an existing compatible base

Use a separately root-signed administrative bootstrap kit built from reviewed
sources, not an ordinary app update. The base must already expose compatible
`requests/control.sock` with `[updates].root`. Stage private root-owned updater
configuration separately. Inventory every writer service and external writer.
Long-running resumable service units are supported; oneshot/RemainAfterExit and
hidden environment/startup overrides are not a safe substitute.

Nginx needs the updater's server include and must clear
`X-Talocart-Update-Probe` before proxying application requests. The private HTTPS
probe requires original local TCP peer plus a root-private token; forwarded IPs
cannot bypass maintenance. Default placeholder configurations deliberately block
deployment until real host settings/fences are asserted.

```sh
/usr/bin/python3 -I bootstrap_updater.py --config-source /root/staging-update.toml --engine /root/talocart-updater-ENGINE_VERSION.tar.gz --signature /root/talocart-updater-ENGINE_VERSION.tar.gz.sig --engine-version ENGINE_VERSION --root-public-key /root/trusted-root-public.pem --nginx-site /etc/nginx/sites-available/shop
```

Verify kit/engine signatures independently before execution. Inspect preview,
then use `--apply` to install infrastructure and test the short maintenance gate.
Bootstrap does not deploy application code or migrate SQL. Retain private copies
of replaced configuration and verify active scheduling.

## Publish and approve locally

Before costly qualification run dependency audits on the exact lockfiles:

```sh
python3 scripts/audit-dependencies.py
npm audit --audit-level=high
```

The audit wrapper checks the active workspace graph (normal, build and development
dependencies on all targets) before excluding only `RUSTSEC-2026-0459` for the
unused SQLite entry in SQLx’s lockfile. If SQLite becomes active, the full audit
applies automatically. Graph resolution failures stop the audit; other advisories
and maintenance warnings remain visible. This covers the default workspace
features used by these builds, not arbitrary downstream feature combinations.

Install the reviewed audit tool if absent; CI pins its version in the workflow.
Resolve applicable vulnerabilities; record maintenance warnings separately without
suppressing them. Then follow the [test policy](../testing-policy.md), build from
the final tag, verify notices, Linux execution, final digests, signatures, install/
upgrade/restore, failure scenarios, benchmarks and human review as applicable.

Prepare/validate distribution metadata with `scripts/release/distribution.py`.
Use an empty module catalog for a base-only release, actual final platform
artifacts and distinct trusted root/application keys. Inspect exact CLI options
with `--help`; secrets stay outside repository/artifact directories.

```sh
python3 scripts/release/publish.py candidate-unified --repo OWNER/REPOSITORY --manifest /private/reviewed-distribution.json --artifacts /private/final-artifacts --key /private/update-private.pem --root-public-key /private/updater-root-public.pem --verified
python3 scripts/release/publish.py approve-unified --repo OWNER/REPOSITORY --tag vX.Y.Z --public-key /private/update-public.pem --root-public-key /private/updater-root-public.pem
```

These are separate explicitly authorized external publication actions. Candidate
uploads a draft after matching remote tags/provenance and checking final artifacts;
approval reads back/verifies the draft before making it available. Neither deploys
to servers. A source beta need not publish approved production binaries/feed.
Do not point the public initial snapshot at private tags/releases/history.

## Owner update, fences and recovery

The owner reads authorized status then POSTs the exact installable proposal. The
agent revalidates signed proposal/sequence watermark, actual registry, configuration
fingerprint and database ledgers before changes. Read-only planning starts no
workers and creates no purchase sessions. A configuration file changed without
restart is refused before quiescing packages.

Deployment records durable intent, enters maintenance, fences writers/restarts,
backs up/verifies, suspends packages, migrates/applies replacements as the
application account, runs doctor/read-only verification, resumes compatible
packages and probes ordinary startup/HTTPS before clearing fences. Application
CLI runs in a bounded systemd cgroup as service UID/GID with a clean environment,
never root. No external provider calls run in SQL transactions.

`serve-verify` is read-only with no job executors, package serving, ordinary session
creation or write endpoints. Its readiness is `verification-ready`. Login/full
cart probes occur only after controlled ordinary activation while public access
is still in maintenance. Promote verified artifact bytes, not rebuilt copies.

Interrupted attempts remain fenced. A launcher can recover without importing the
selected engine, using only the current root-owned bounded journal/backup checkpoint.
Unknown errors expose generic public status, never private paths/config/secrets.
Do not erase markers, invent a verified backup or manually start writers to
hide an incomplete attempt. Follow administrative recovery with the verified
coordinated backup and fresh destinations from [operations](../release-operations.md).
Uncertain external payment/mail effects require separate reconciliation.

## Qualification

Use disposable Linux/systemd resources and synthetic data; never automated tests
on a real store. `scripts/test-update-systemd.py` and
`scripts/test-update-integration.py --help` describe explicit smoke/failure paths.
First-install scenarios require fresh containers and database, TLS, kill, reboot
and disk-full failures. Preserve data and assert failed/incomplete startup stays
blocked even without writable logs/markers. The repository's presence of these
tools is not evidence they ran on the current release or destination.

## Native plan and offline primitives

The **target** executable can inspect installed package compatibility:

```sh
/path/to/target/talocart --config /private/application.toml package-update-plan --package /private/target-module.talopkg --check-database
```

Omit `--package` to check reuse, or repeat it for up to 32 replacements. The plan
checks content integrity, contracts and migration prefixes (plus embedded
signature consistency for legacy module archives) and
reports `keep`, `update`, `blocked` or uninstalled `ignored` IDs. Equal versions
require identical contracts and executable bytes; legacy publisher metadata is
ignored for this comparison. It does not launch workers, change registry state
or acquire the host lease. Inventory changes invalidate the read.
Without `--check-database`, `database_checked=false` is not permission to install.
With it, a repeatable readonly snapshot checks base/private ledgers and returns
a schema digest, excluding mutable commercial rows. Recheck before quiescing
and under the deployment locks.

Offline `package-update-suspend`, `package-update-migrate`, `package-update-apply`,
`package-update-verify` and `package-update-restore` are coordinator primitives,
not a shortcut around the signed plan. They require the host stopped, exclusive
lease, service-account execution and a verified coordinated backup. Suspending
enabled modules requires the live manager's inventory-bound quiesce snapshot
through `--quiesced` stdin. Preserve the returned original snapshot in the journal
for restore; never reconstruct it by editing the registry. Verification uses
readonly database access and `CanStop`, without starting jobs or package serving.

The private control socket is `requests/control.sock` under `[updates].root`:
0700 directory, 0600 socket, kernel-verified root peer. Its typed messages allow
inventory, digest-bound quiesce and resume, not shell commands or arbitrary paths.
The updater checks the application UID. Every action checks the TOML bytes loaded
at boot; editing the file without restarting does not update the live contract.
Pending jobs block quiesce; an uncertain resume retains the barrier.

The full plan also binds actual units and privileged configuration using a local
root-private secret so public digests cannot be used to guess configuration secrets.
Changed plans require renewed confirmation. Failed artifact preparation does not
stop the store; failures after mutation retain fences and verified backup evidence.

## Privileged runtime details

The base service must use systemd `simple`/`exec`; other resumable writer services
may use `simple`, `exec`, `notify`, `dbus` or `forking`, with `RemainAfterExit=no`.
Do not use hidden `TALOCART_*`, `PG*`, `Environment` or `EnvironmentFile` overrides
instead of complete reviewed configuration. Base startup hooks and conditions must
respect the managed guards. Independently inventory cron and remote writers.

Stage `distribution/update-unified.toml` as a separate root-owned 0600 file.
The root signing key must differ from application keys and any legacy package publisher keys. Use
the packaged coordinator, which embeds its helper sources; raw source alone is
not the bootstrap kit. No private signing key belongs on the target server.

Nginx HTTP configuration must load the managed includes; every proxy location
must clear `X-Talocart-Update-Probe`, including locations with their own headers.
Replace any older unconditional maintenance `return 503` gate with the reviewed
managed include so the authenticated local probe can work. Validate before reload.
The probe verifies TLS/SNI and original local TCP peer; forwarded headers do not
supply authorization.

Signed engine handoff runs a separate process after ownership, signature, digest
and self-check verification. Incomplete application updates block engine switching.
The launcher and trust roots are outside engine bundles. Interrupted publication
uses private verified receipts; it cannot reuse a version for different bytes.
A failed self-check blocks that engine digest until a corrected artifact is approved.
An engine-only update with unchanged application components does not require
application SQL or an additional application backup.

The canonical maintenance page is `assets/maintenance.html`; packaging embeds it
in the signed coordinator. It always uses the light storefront palette and native
sans-serif typography, including when the visitor prefers a dark system theme.
It remains self-contained so it can render while the application is stopped.
`python3 scripts/build-maintenance.py --check` verifies
its CSP. Before stopping writers, the new engine updates only its managed Nginx
snippet, preserves the token/gate, validates and reloads; rejected configuration
is restored. Deploying only the base does not update an older engine's page.
Manual store maintenance remains in the database and survives upgrades; private
verification probes must retain their attestations even on manual 503 responses.

## Linux integration commands

Use disposable infrastructure and compatible final Linux artifacts only:

```sh
docker build --platform linux/amd64 -f tests/release/Dockerfile.integration -t talocart-updater-integration:ubuntu24-pg17 tests/release
python3 scripts/test-update-integration.py --release /private/extracted-base
python3 scripts/test-update-integration.py --release /private/extracted-base --archive /private/base.tar.gz --fixtures /private/linux-fixtures --faults
```

The first command path tests clean installation, live control and signed bootstrap;
the second adds a keyless synthetic module and failure/restore scenarios. Fixture
executables must match Linux architecture and host contracts. The runner uses a
disposable systemd container with synthetic PostgreSQL, HTTPS, accounts and keys.
`--initial-release` selects a supported compatible predecessor; `--base-only`
retains installed packages. The initial fixture must match the predecessor;
replacement fixtures must match the target host.
When the predecessor requires historical module keys, the test harness uses its
old builder/registration only to prepare that initial state; replacement packages
are built by the target host without keys. `--final-engine` selects the actual signed engine
bundle and public key, with no production private keys in the container.

`--first-install` and `--first-install-failure database|tls|kill|reboot|disk-full`
exercise initial provisioning and fail-closed startup. For interruption coverage,
inspect `scripts/test-update-interruptions.py --help`; individual cases use
`--interrupt-at` and `--interrupt-mode kill|reboot`. Hooks belong only to disposable
signed test copies. Verify recovery after corrupting the selected engine too.

Qualify each supported update path with final artifacts from the version-15
baseline or later. Historical development schemas are not supported.

## Unified-only updater

Only an explicit `feed_format = 2` configuration is accepted. The retired
format-1 agent cycle, deployment path and `candidate`/`approve` publication
commands are not supported; use `candidate-unified` and `approve-unified`.
Application archives contain no updater executable or updater service/configuration.
Install the separately signed engine through the reviewed administrative bootstrap,
which is also used by first installation. The engine still shares verification,
transport, extraction and recovery helpers; their Python filenames do not denote
an older supported feed format. The `bootstrap_updates` conversion option is removed.

### Strict module readiness

Updater loopback and HTTPS readiness requests append `?all_modules=true` to
`/health/ready`. This preserves complete enabled-module checks even when an owner
allows the storefront to remain available without an optional module. Older hosts
ignore the query and already require all enabled modules. Offline verification
still validates every installed module, including disabled ones. Deploy this
updater behavior together with hosts supporting configurable module criticality.
