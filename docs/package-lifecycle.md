# Native package lifecycle

[Package contracts](installable-packages.md) define validation/compatibility. This
document defines persistent transitions, locks and recovery in the base manager.

| Component | Responsibility |
| --- | --- |
| `format`, `build` | Validated manifests, bounded archives and packaging |
| `manager/model` | Registry model, previous release, intent and preview |
| `manager/storage` | Private files, filesystem locks, atomic publication and pruning |
| `manager/validation` | Descriptor, integrity, conflicts, compatibility and ledgers |
| `manager/process` | Handshake, process token/start, quiesce/resume and shutdown |
| `manager/supervision` | Enabled-process startup, bounded recovery and sanitized runtime status |
| `manager` | Transition orchestration and persistence ordering |
| Web packages | Owner/CSRF, EN/IT presentation and authorized proxy |
| External module | Its own rules, state, settings, jobs and schema |

Per-ID reader/writer gates isolate packages from each other, so unrelated packages
never queue behind slow startup. Administrative GET/HEAD requests and payment
calls share their package's gate and run concurrently. Other administrative
methods and lifecycle transitions own it exclusively and wait for in-flight
requests, including payment leases retained through host SQL commit. The gate is
fair: later readers queue behind a waiting writer. Shared and exclusive request
admission wait at most 10 seconds, then refuse the request as unavailable. Modules
also serialize their own handlers to drain work after a host timeout. A
module-requested restart releases its own request first and stops only the process generation that answered.
The global operation lock protects registry transitions, not ordinary
proxy/startup. Recheck authorization, routes and generation after acquiring the gate. Memory locks do not replace durable intents.
Update confirmation obtains only the verified ID from preview, locks it, then
rechecks owner, expiry and payload under operation/disk locks.
The maintenance barrier refuses new admission; a lifecycle operation already
holding its gates must finish before maintenance takes ownership.
Before the first quiesce attempt, a preparation guard reopens admission on every
error, including disk/registry verification failures. Once a child may have been
quiesced, only confirmed resume releases the barrier; uncertain control outcomes
retain the paused state for recovery.

| State | Behavior / next action |
| --- | --- |
| Absent | No routes; earlier private data can remain; upload and preview |
| Preview | Expiring owner-bound token, no migration; explicitly install/update |
| Installed disabled | Verified executable/manifest, no serving; enable or uninstall |
| Active compatible | Host starts and supervises child independently of HTTP traffic |
| Active child exited | Registry preserved; bounded retries recheck integrity/handshake |
| Active child unresponsive | Readiness fails; probe recovery preserves the same process and restart budget |
| Repeated failures | Three consecutive failures; state `failed`, host retries once per minute; owner checks diagnostics and may restart now |
| Damaged | Executable missing or not matching its manifest at registry load; listed, never executed, cannot be enabled; disable, uninstall and reinstall or install a newer version |
| Incompatible | Listed, never executed; doctor rejects active incompatible entries |
| Incomplete update | Durable intent, possibly intermediate private ledger; retry/release correction |
| Updated | New current release, previous retained, intent removed |
| Uninstalled | Registry/executables removed; data/schema/settings and legacy metadata preserved |

Install verifies content/contracts/compatibility, atomically publishes the
complete executable, checks its actual descriptor before private migrations and
registers disabled. Update requires the same ID and a newer version with compatible
history, then publication, descriptor verification, quiesce, durable intent,
migrations, completion and optional start. Neither flow requires a publisher key.
Retries revalidate against the current host. Returning to the previous executable
is allowed only when its exact ledger remains valid. No destructive SQL rollback.

## Failure handling

- Refused quiesce for pending work preserves usable process/registry.
- Lost quiesce response attempts resume and reports uncertainty, without silently
  killing a package considered busy.
- Browser disconnect does not abandon an authorized mutation task; reload reads
  final persisted state.
- Child exit requires renewed integrity/compatibility/handshake at startup; parent
  stdin EOF shuts down orphaned children.
- Intentional stop/update/deactivation does not count as a crash. Supervision
  respects per-ID gates and the coordinated-update barrier; shutdown fences new
  starts before draining requests. It never changes persistent desired activation.
- After three consecutive start/exit failures the host keeps retrying once per
  minute and journals one error line per distinct phase/cause, not per attempt.
- A missing or modified executable marks only its own entry damaged when the
  registry is loaded; the host, doctor and package commands still open. Registry
  tampering remains fatal, and coordinated update planning, preparation,
  suspension, activation restoration and warm-up refuse a damaged inventory.
  Restoration and warm-up verify disk integrity even for disabled modules;
  the serialized registry fingerprint alone does not attest executable integrity. `package-disable` and `package-remove`
  recover it while the host is stopped.
- Returning to a verified previous release clears the rejected executable's retry
  budget, so its failure does not postpone recovery of the restored process.
- Process restart is owner/CSRF protected and requires quiesce for a live child.
  Busy work preserves its process and registry. A dead child can recover pending
  jobs without pretending that work has completed.
- SDK 6 health probes authenticate independently of request gates. A two-second
  probe timeout or a result older than five seconds fails readiness without
  restarting the child; results from a replaced process generation are discarded.
- Interrupted writes never publish partial executable payloads; same-directory
  temporary files, sync and non-overwriting publication protect the destination.
- Interrupted migrations retain ledger/intents for conservative retry. Incomplete
  state is not hidden by launching incompatible previous code.
- Recovery treats database, media, packages, secrets and compatible binaries as
  one coordinated unit, with hashes and new target paths.

Preserve unit manager/format/protocol tests, `crates/package-fixture/tests/updates.rs`,
HTTP package tests and `scripts/test-package-updates.cjs`. External module-specific
suites stay in their own repositories. Evidence must identify actual executables;
test source presence alone does not prove execution.
