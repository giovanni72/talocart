# Candidate operations checklist

Target: one store on PostgreSQL 17, private persistent local storage and a
loopback listener behind HTTPS. Use [operations](release-operations.md) and the
[test policy](testing-policy.md). Publishing sources is not production approval.

## Candidate identity and qualification

Identify baseline/candidate diff, final artifacts/digests, supported platforms,
upgrade paths, package contracts, test selection, exclusions and human review.
First production entry requires full coverage and complete operational drills;
later releases use impact classification and mandatory minimum checks.

Initial small-store measurement target: five concurrent confirmations, local
p95 within 1,000 ms and no technical failures. Concurrency 10/20 characterizes
headroom rather than a production SLA. Last-item contention may return commercial
conflicts, but permits exactly one sale without partial effects.

Recover transient database failures without mandatory process restart. Committed
mail jobs survive restart/outage and resume. SMTP does not guarantee exactly-once
delivery after a lost acknowledgement; preserve Message-ID and reconcile uncertainty.

## Installation

1. Verify trusted provenance, signatures/digests, platform and build identity.
2. Create private service configuration, dedicated database and distinct persistent
   media/package/log directories with correct ownership/modes.
3. Configure exact HTTPS origin/secure cookies, loopback binding and operator-only
   access until one-use setup completes.
4. Explicitly migrate, doctor, start, probe live/ready, browse EN/IT and log in.
5. Check SMTP, selected commerce methods, themes and human mobile/desktop rendering.
6. If using the full installer, verify base plus updater scheduling and all
   first-install failure guards on disposable Linux environments.

## Upgrade

1. Inspect active version, schema/prefixes, package contracts and disk capacity.
2. Resolve pending module work, deactivate packages, fence every writer/restart.
3. Create and verify a coordinated database/media/packages/secrets/config/binary backup.
4. Stage a new verified release directory. Migrate explicitly, doctor and switch
   only after successful checks. Reuse compatible unchanged module executables.
5. Verify readiness, HTTPS, login, assets, core commerce and affected behavior.
6. Inspect stock/orders/queues and remove maintenance only after accepting health.

## Recovery and diagnosis

Preserve uncertain state and artifacts. Restore a verified coordinated backup
only into a new empty database and new storage paths with explicit target config
and compatible executable. Compare tables/ledgers/hashes/private modes and browse
EN/IT, accounts and commercial snapshots before accepting recovery. Integrity
verification alone does not prove a restore. Never reset or overwrite application data.

Reconcile external provider and SMTP effects before resuming workers; database
restore cannot undo accepted payments/deliveries. Preserve complete files after
SQL failure/timeout because commit outcome may be uncertain.

Live-positive/ready-negative means inspect configuration/schema/dependency health,
not run migrations through probes. Incomplete packages retain intent/ledger for
retry/correction. Journal failures and full disks must not bypass startup fences.

Check `talocart::checkout` timing, `talocart::workers` lease/queue metrics and
transaction retry events. Pool wait includes preflight and BEGIN round trip,
not just queue time; aborted is not proof of rollback acknowledgement. Missing
diagnostics are explicit and must not prevent service operation.

Define environment-specific monitoring, backup retention, recovery cadence and
responsible operator. Machine tests, visual approval, CI, publication and actual
deployment remain separately evidenced milestones.
