# Installation, releases and recovery

This guide describes operational contracts for a development candidate. It does
not approve real shops. Use the [test policy](testing-policy.md) and
[checklist](operational-readiness-checklist.md) to qualify final artifacts and
the actual environment. Keep the listener on loopback behind an HTTPS reverse proxy.

## Supported layout

Linux x86_64/arm64, glibc 2.36+, libc/libgcc, CA certificates and PostgreSQL 17.
Maintenance requires Python 3.11+ and PostgreSQL 17 clients (`psql`, `pg_dump`,
`pg_restore`). The executable
embeds templates/assets/translations/demo/migrations and needs no Rust or Node.

Release archives contain `bin/talocart`, `release.json`, examples, operational
tools, MIT and third-party notices. The manifest records platform, version,
commit, build identity and hashes. Checksums prove integrity against a trusted
reference, not authenticity; use independently trusted signatures/release channels.
Use new versioned directories, configuration in `/etc/talocart` and persistent
state under `/var/lib/talocart`, outside releases. Never extract over an old release.

## Manual installation

For an independently signed complete installer, read [application updates](guides/application-updates.md).
The ordinary application archive does not itself provision the updater.

1. Verify the trusted archive/checksum and extract into `/opt/talocart/X.Y.Z`.
   Substitute the verified version in example paths. Verify `release.json` from
   the extracted directory:

   ```sh
   python3 -c 'import sys; sys.path.insert(0,"scripts/release"); from operations import verify; from pathlib import Path; verify(Path("."),"release.json")'
   ```

2. Create an unprivileged service account, private persistent directories (0700),
   private config (0600) owned by it and an empty dedicated PostgreSQL database.
   Set credentials privately, outside history/logs/process arguments.
3. Adapt `distribution/talocart.https.toml`: exact HTTPS origin and secure cookies
   together, loopback listener, absolute media/packages/log paths. SMTP is initially
   disabled and configured after setup. Countries/shipping/payments need explicit settings.
   The signed first-install command requires explicit `media.root`, `packages.root`
   and `logging.root` at `/var/lib/talocart/media`, `/var/lib/talocart/packages` and
   `/var/lib/talocart/logs`. Omitted or relative roots resolve beside the application
   TOML and are rejected before installation rather than assigned installer defaults.
4. Run the selected executable as the application account:

   ```sh
   /opt/talocart/X.Y.Z/bin/talocart --config /etc/talocart/talocart.toml migrate
   /opt/talocart/X.Y.Z/bin/talocart --config /etc/talocart/talocart.toml doctor
   ```

5. Point `/opt/talocart/current` at the checked release, adapt/install the systemd
   example, validate it, daemon-reload and start. Probe `/health/live` and
   `/health/ready`; the service never migrates. Inspect private journal diagnostics.
6. Adapt Nginx DNS/certificates and reject unknown virtual hosts. Restrict all
   access to the operator until `/setup` creates the owner and closes. Validate
   Nginx before reload. Never expose PostgreSQL, port 3000 or storage publicly.
7. Verify login/logout, cookie attributes, EN/IT, themes, assets and synthetic
   checkout. Align configured public URL/origin. Capture test email before real SMTP.

The reverse proxy sends the configured Host. Unexpected Host/Origin are rejected;
forwarded headers cannot arbitrarily change destinations or authorize clients.
Client IP forwarding is not trusted. With a configured HTTPS origin and secure
cookies, the proxy owns per-client customer authentication limits; loopback peers
are not counted as one shared customer. The supplied Nginx configuration limits
the login, registration and recovery routes across EN/IT to 12 requests/minute
per client IP with a burst of 20 and HTTP 429 on excess. Install and verify those
rules together with the HTTPS application configuration. Session/account limits
remain enforced by application services. HTTP local preview and non-loopback
test peers retain transport-origin limits. If another proxy precedes Nginx,
configure its trusted-client contract explicitly rather than accepting arbitrary
forwarded headers. See Nginx's [limit_req contract](https://nginx.org/en/docs/http/ngx_http_limit_req_module.html).
HTTPS does not supply an external payment provider.

## Coordinated backup

Fence **all** writers/restarts for the entire window: app, external jobs and
other clients. A database connection check does not replace operator fencing.
Use a complete private TOML rather than hidden environment-only settings and
keep backups outside media/packages. The output must be new.

```sh
python3 scripts/release/operations.py backup --config /etc/talocart/talocart.toml --binary /opt/talocart/current/bin/talocart --output /private-backups/new-backup
python3 scripts/release/operations.py verify /private-backups/new-backup
```

Run as the application account with PostgreSQL 17 tools. Stop/fence services
before these commands and resume only after checking the complete outcome.
Short checks have 30-second timeouts; dump/restore default to 3,600 seconds,
overridable with positive `--database-timeout 7200`. Timeout is not proof of rollback.

The tool checks schema/binary compatibility and coordinates a custom-format dump,
media, installed package registry/executables/settings, SMTP secret key, config
and compatible binary. Hashes/private modes are checked and all files/directories
are synchronized before publishing the backup; its parent is synchronized after
the rename. New parent directories are synchronized too. Directories are 0700,
private files 0600 and saved executables 0700. A synchronization error never reports
success. If it occurs after publication, the output remains for inspection with
uncertain durability. Failure preserves the source and leaves diagnosis/restart
to the operator.

Backups contain credentials/session hashes/personal data. Restrict/encrypt off-host
copies and choose retention. Database roles/credentials, TLS, service environment,
external secret stores and remote provider state need separate private recovery
procedures. Backups cannot reconstruct files never included. Integrity is not
authenticity or proof of a successful restore.

## Restore and rollback

Keep writers fenced. Restore only to a **new empty database and new media/package
paths** using explicit private target TOML and the compatible backed-up binary.
Never drop/overwrite a live database or rewind migration ledgers.

```sh
python3 scripts/release/operations.py verify /private-backups/new-backup
python3 scripts/release/operations.py restore --backup /private-backups/new-backup --config /etc/talocart/recovery.toml
/private-backups/new-backup/talocart --config /etc/talocart/recovery.toml doctor
```

Files are staged in private sibling directories, fully verified and synchronized
before publication. Each publication's parent is synchronized before transactional
`pg_restore --single-transaction` starts. Copy or synchronization failure does not
start SQL; publication failure retracts only this invocation's directories, attempting
all retractions even if one synchronization fails. Cleanup failure requires inspection
of the new paths before retrying. After SQL begins, failures/timeouts preserve
complete files because a client failure cannot prove whether the server committed.
Inspect state with writers still stopped; use fresh destinations for a new attempt.
The restore tool starts no workers.

Compare all tables, ledgers, hashes, private modes, references and settings. Start
the compatible executable in isolation and verify readiness, EN/IT, owner login,
orders/stock and themes. Keep the old environment fenced until accepting recovery.
Database rollback cannot undo accepted external payments or SMTP deliveries;
reconcile before workers resume. Changes after the backup are the rollback window.

## Upgrade

Read compatibility/release notes, stage a verified new release beside the old,
resolve pending package work and deactivate packages before base migrations.
Fence writers, back up/verify, run new migrate then doctor, switch/start and smoke
HTTPS/login/commerce/EN/IT. Reuse compatible unchanged package executables; replace
only changed/incompatible modules. Never run an older binary against incompatible
new schema; restore a coordinated backup into new destinations instead.

The separate updater implements signed owner-requested deployment with persistent
maintenance/fences and recovery. Publication does not install anything. Follow
[application updates](guides/application-updates.md) for provisioning and failures.

## Build and package

```sh
python3 scripts/release/version.py check
cargo build --release --locked -p talocart
mkdir -p .local
cargo metadata --locked --filter-platform x86_64-unknown-linux-gnu --format-version 1 > .local/metadata.json
python3 scripts/release/package.py --binary target/release/talocart --target x86_64-unknown-linux-gnu --metadata .local/metadata.json --output .local/dist
```

Use the actual executable target. Optional `--version` asserts Cargo equality;
it does not define a separate version. Official packaging uses clean annotated
main/tag, rebuild and `--release`. Packaging verifies version/commit/fingerprint,
pinned active compiler and composition. It does not prove which compiler produced
an arbitrary supplied executable. Metadata may contain private paths and is not
itself a public artifact.

Linux builds use the pinned Rust bookworm image recorded in the release workflow.
Build natively or explicitly emulate the target. A clean rebuild comparison in
the same pinned environment/source path plus two archive comparisons measures
that setting, not universal reproducibility. Archives normalize order/ownership/
modes/timestamps. Do not mount private runtime files into build services.

## Acceptance

```sh
python3 -m unittest discover -s tests/release -v
node scripts/release/acceptance.cjs
```

Acceptance requires private `TALOCART_TEST_DATABASE_URL` for a dedicated synthetic
database role with CREATEDB, PostgreSQL 17 clients, Node/browser dependencies and
free local ports. `TALOCART_TEST_BINARY` can select the extracted candidate;
`TALOCART_RELEASE_OPERATIONS` can select its packaged maintenance script.
`RELEASE_TEST_PG_CONTAINER` selects the dedicated Docker client adapter.
The drill creates disposable whole databases, checks clean setup/repeated migrate,
order/stock preservation, complete backup/restore/table/media comparison, failed
restore preservation and HTTPS login EN/IT with/without JS. It does not touch
the application's ordinary database/configuration/media. Final Linux and first-install
qualification require additional explicit drills in the test policy.

## Error journal

Serve writes ERROR events to private `errors.YYYY-MM-DD.jsonl`, rotating on the
first write after midnight UTC. `logging.max_files` accepts 2–365 daily files,
not a calendar-age guarantee. Console JSON remains on stderr with its own filter;
non-serve CLI stdout remains for results. Errors before journal initialization
remain on stderr. Not every validation error generates a journal event.

Bounded asynchronous writing isolates HTTP from filesystem latency. The owner-only
`/admin/error-logs` service reads bounded data without public file routes or cache.
Never publish raw journals without inspecting them for sensitive diagnostics.
Size/storage monitoring and access protection remain operational responsibilities.

### Journal storage and SMTP recovery details

Use a dedicated log directory: pruning selects the `errors` prefix and `jsonl`
suffix at startup/rotation, and files append across same-day restarts. There is
no byte quota or file for days without events. Relative paths resolve from the
TOML directory; Unix directory permissions are 0700. Keep logs outside media,
packages and public routes. Size monitoring remains necessary.

The SMTP encryption key in `<media-root>.secrets` is recoverable private state.
Preserve it with the database and configuration through the coordinated tools;
a database dump alone cannot decrypt transport credentials. Test missing/wrong-key
refusal and restore into new paths before enabling delivery. Keep the previous
instance fenced to avoid duplicate external sends.

Installed package backup includes the registry (including legacy metadata), executables and
module settings. Restore into a new package root distinct from media. Module
executables regain 0700 and private settings 0600. Older backups cannot restore
package files that were never saved. The HTTPS example allows a 128 MiB package
payload plus envelope only on the exact upload route (129 MiB proxy limit);
other routes retain their normal 12 MiB limit.
