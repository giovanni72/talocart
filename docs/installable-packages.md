# Installable native packages

The owner can preview and install `.talopkg` executables without registering keys
through Modules → Installable packages. No compiler or replacement of the base is
needed to install an independently supplied native extension. This repository
includes the host interfaces and synthetic fixtures, not external module code.

## Validation and compatibility

Pre-installation checks validate the declared contracts, payload integrity and
compatibility; they do not certify code safety or publisher identity. Native
packages run with application access and are not sandboxed. Preview reads the
archive without executing it. On explicit installation confirmation, the host
runs the executable's bounded `--describe-package` with an empty environment and
no supplied database credentials, comparing all contract fields before passing
credentials or invoking migrations. Obtain executable code from a source you trust.

Current contracts are SDK **6.0.0**, handshake protocol **3**, manifest format **4**
and base database contract **4**. SDK compatibility accepts a previous nonzero
minor/patch in the same major, never a future SDK. Protocol/database contract and
OS/architecture must match exactly. Application SemVer is independent.
Packages built against earlier SDK majors cannot execute; rebuild and package them
against the current SDK. Administrative paths must be confined to
`/admin/extensions/{id}` without package-specific exceptions. Legacy manifests
outside that namespace are rejected, including when loading installed registries.
Remove such packages using the previous host before upgrading, after completing
pending work and verifying a backup; preserve private package data for reinstall.

The declared database checkpoint contains contract version, required migration
count and canonical digest of migration IDs/SHA-384 checksums. It must be an
exact prefix of the host's compiled history. On startup a module validates the
actual complete host ledger and its own exact private ledger. Missing, unexpected,
changed or failed migrations are refused. Checks do not repair SQL or checksums.
Legacy pre-format-4 manifests/registries are not accepted by this generation.

Increment SDK major for breaking APIs, protocol for incompatible wire messages
and database contract for incompatible shared SQL semantics. Compatible additions
use new migrations without rewriting distributed prefixes. Reuse an unchanged
previous module executable to test compatibility; rebuilding it with the host
does not prove old-module compatibility. New embedded module fixes require new
module releases even when contracts remain compatible.

## Package and install

An external provider builds its own executable against the published interfaces.
The host CLI packages executables without reading the application database:

```sh
./target/debug/talocart package-build --executable /external/module --output /distribution/example.talopkg
```

The `TALOPKG2` envelope contains the bounded manifest and executable, with no key
or signature. Identity, platform, routes, capabilities, payload size and SHA-256
are checked. Hashes detect corruption; they do not authenticate a supplier.
The registry refuses ID/path/ledger conflicts and incompatible packages. Preview
is owner-bound, expiring and read-only; install/update requires an explicit POST
with current owner authorization, CSRF and current-password reauthentication.
`packages.install` supplies the initial default (`"cli"` when omitted). In
Settings → General, the current owner can choose `"cli"` or `"panel"`; changing it
requires current-password reauthentication. The choice is saved atomically with
other settings and persists across restarts, overriding the file default. Each
module page/action reads it without caching, so no restart is needed. CLI mode
hides upload/retry forms and forbids upload, install, update and retry POSTs.
Administrators with only delegated settings permissions cannot change the mode. Read-only inventory and ordinary
enable/disable/restart/uninstall remain available. Password verification shares
the account reauthentication budget (10 attempts per rate window), rechecks the
current session/role/password, and does not keep a SQL transaction open during
module execution. Installation publishes a complete executable
atomically, compares its actual descriptor, checks private schema and initially
registers it disabled. A mismatched descriptor is rejected before private DDL.

For administrative offline installation with the host stopped and verified backup:

```sh
./target/debug/talocart --config /private/talocart.toml package-install --package /distribution/example.talopkg --enable
./target/debug/talocart --config /private/talocart.toml package-update --package /distribution/example-next.talopkg
./target/debug/talocart --config /private/talocart.toml package-retry --id example
```

These commands hold the exclusive host lease and uses the same manager checks. They do not start host workers. The package
still needs a valid implementation and platform-specific executable.

### Existing packages and registries

Compatible `TALOPKG1` archives and installed signed releases remain readable.
Their embedded signature is checked for consistency, without consulting a trusted
publisher list. Existing registry metadata and private data are preserved; old
keys need not be registered or recreated. Legacy Rust helpers remain available
for source compatibility, but the host CLI/UI no longer exposes key generation or
publisher registration. Updates require the same module ID, a newer version and
an unchanged migration prefix; publisher identity is not an update gate.
Repacking identical executable/contract bytes without a signature does not count
as a module upgrade. Older hosts cannot read `TALOPKG2`; downgrade requires the
coordinated backup and compatible previous artifacts, not registry editing.

The archive envelope is independent of the SDK, lifecycle protocol, manifest and
database contracts. Coordinated application-distribution and privileged-updater signatures
have a separate purpose and remain required; removing module keys does not change
their verification, session authentication, CSRF or process tokens.

## Process and payment capabilities

Processes communicate over authenticated loopback with bounded messages,
timeouts and explicit handshake. Routes/assets are restricted by validated manifest
namespaces. Current permissions and lifecycle generation are rechecked under
the per-ID gate. Modules own private settings and EN/IT rendering while shared
identity, UI shell and commercial services remain host-controlled.

Administrative GET/HEAD requests and payment calls share the per-ID gate and
run concurrently. Other administrative methods and lifecycle transitions
(enable/disable, restart, update, uninstall, coordinated preparation) own it
exclusively and wait for in-flight requests and host payment transaction leases.
This prevents settings changes between provider validation and SQL commit; a
module-local handler lock alone cannot protect transactions inside the host.
The gate is fair: a request arriving behind a waiting exclusive operation queues after it. Both shared and
exclusive request admission wait at most 10 seconds, then refuse the request as
unavailable (503 for proxied pages). A module whose handlers must not overlap
serializes them in its own process, for example with a reader/writer gate where reads share and
mutations and `/__talocart_package/control` are exclusive, as both SDK 6
modules do. This changes no SDK, protocol or database contract version.
A response with `x-talocart-module-restart: 1` makes the host release that
request, wait for the module's other requests and stop only the process
generation that answered.

Every enabled, compatible package is started automatically by the serving host,
including after a host restart. No page visit is required to resume durable jobs.
Modules must supervise required internal workers: unexpected worker termination
must fail the process so the host can recover it. Jobs must use durable leases and
idempotent writes; process restart alone does not guarantee exactly-once work.
Doctor, offline commands and `serve-verify` do not enable the supervisor.

The host checks processes every second. Retries after a startup failure/exit wait
2 then 4 seconds; after three consecutive failures the state is `failed` and the
host retries once per minute until the module starts or is deactivated. An explicit
restart remains available at any time, and 60 seconds of stable service reset the
failure count. The host journals one error line per distinct phase/cause while
slow retries repeat it. Each attempt rechecks executable integrity and
the handshake. Other packages retain their independent gates. Readiness is 503
while an enabled package is unavailable, starting, waiting, failed or not
responsive; authenticated administration remains accessible for recovery. A process being alive does not
prove external provider availability. The existing payment startup validation
still fails host startup if pending-payment configuration cannot be recovered.

SDK 6 requires `GET /__talocart_package/health` on the private listener. Validate
the host token before returning 204 with an empty body; other methods return 405
after authentication. This endpoint must bypass request/quiescence gates and must
not query the database, contact providers or change state. The host schedules
probes on one-second supervisor ticks with a two-second deadline, outside the
ordinary request gate; at most one probe per package runs at a time. Readiness
requires a successful result from the current process generation within five
seconds. Probe failure never kills a live process or spends its restart budget;
the same process can recover readiness. This checks authenticated HTTP
responsiveness, not external provider availability or successful business work.
SDK 5 executables lack this mandatory contract and must be rebuilt; the handshake,
manifest and database versions are unchanged. There is no legacy probe fallback.

Modules shows desired activation separately from process state, the last sanitized
failure phase/cause and consecutive failure count. The owner can POST a process
restart with CSRF; a live module must quiesce successfully first, so pending work
blocks the restart. A dead process can restart and recover its durable work.
Diagnostics and retry counters are in memory and reset with the host; private host
logs retain sanitized events. Raw child stderr and credentials are never copied to
the page or host logs. This is separate from retrying an incomplete package update.
`Running` describes a live process; readiness additionally requires recent health
probes of the modules required by its policy (see below), so an unresponsive
process can remain `Running`.

SDK 6 supports optional `x-talocart-navigation` request context. The host overwrites
any client value before authenticated forwarding. The standalone SDK router reads
it only behind the module token gate and only for administrative extension paths.
It carries version 1, locale, owner-navigation availability and the visible native
menus; it contains no credentials and grants no authorization. Modules reload the
session and filter links against its current permissions. Handlers retain their
own authorization and CSRF checks. Context is request-local, bounded to 28 KiB and
32 menus, with titles limited to 128 characters and validated namespace paths. The v1
permission-bit positions in `web/src/extension_context.rs` are immutable.
Unknown/invalid/absent context falls back to the local module shell. The header
grants no compatibility exception to executables from an earlier SDK major.

Optional payment `amount_limits` is a nonempty map of uppercase three-letter
currencies to positive inclusive `minimum`/`maximum` integer amounts.
A declared map accepts only its listed currencies; invalid maps are rejected at
installation. The provider owns these values; the host contains no provider table.
Capture the limits outside commercial transactions and pass them to `Carts` and
`Orders` with `with_online_limits`. HTTP confirmation captures them under the
provider generation gate and checks the final total before stock/order writes.
Providers without a declared map use the generic method thresholds. New attempt
creation also checks the provider bounds; existing attempts remain recoverable.

A manifest may declare one native payment provider, RPC payments version 2,
a bounded list of webhook headers, checkout destination, retry window and CSP.
Provider identifiers are opaque printable ASCII strings (1–255 bytes); the
adapter validates their semantics. Normalized results carry payment/refund states
and webhook object kinds, not provider-specific intent/charge structures.
Commerce owns persistent attempts/reconciliation/idempotency and gates confirmation
through commit; the module owns network/signature mechanics and normalized results.
Message limits and timeout/gate bounds prevent unbounded waiting. Multiple native
payment providers may coexist; each package ID owns one payment method. Installation
registers it once, using the manifest title as its initial EN/IT name. The method
starts inactive; existing merchant names, instructions, charges and activation are
preserved on reinstall. The host also registers already-installed packages at startup
after checking the migrated schema. Package removal preserves method configuration
and order history. Disabled, missing or unconfigured providers cannot accept new
checkout selections; another provider never takes over their orders.

Migration 0004 preserves the legacy online method ID and settings, binding its
provider from the already-installed package during host registration, before any new
package is registered. Historical attempts may belong to retired providers and never
claim the current method. Multiple historical provider identities remain valid.
New snapshots include the
provider ID; older snapshots use their original attempt or the preserved legacy
method. Stop/startup checks use the same precedence, including expired attempts on
unpaid legacy orders. RPC, webhook routing, assets, presentation, CSP and refunds select that exact
identity. Unqualified calls fail closed when more than one provider is enabled.
SDK 6 and payment RPC 2 remain unchanged; the additive SQL migration extends the
existing database checkpoint. Older host binaries must not open the migrated database.
Secrets are not returned to page HTML.
Checkout pages ask for provider availability on every render; the host reuses a
successful `Status` answer for up to 2 seconds within the same process
generation, for presentation only. Failures are never cached; the cache is
discarded when the process stops, restarts or exits and after any forwarded
non-GET/HEAD administrative request. Confirmation always asks the provider
again, and the provider rejects creation itself when it is not accepting.

## Update and recovery

Before base migrations deactivate packages, fence writers and take coordinated
backup. Install the new base, migrate/doctor and reactivate compatible existing
executables. Replace a module only when it changes or becomes incompatible.
Deactivation is refused while relevant work is pending; do not discard jobs to
force it. Base migrate refuses pending schema changes while packages are active.

The [lifecycle contract](package-lifecycle.md) describes intents and recovery.
Uninstall removes registry/executables, preserving private data/schema/settings.

A registry entry whose installed executable (or interrupted-update executable) is
missing or no longer matches its manifest is marked *damaged* when the registry
is loaded, instead of preventing `serve`, `doctor`, `migrate` and package commands
from opening it. A damaged module never runs and cannot be enabled; readiness and
`doctor` fail while it is enabled. Registry tampering (manifest, raw bytes,
signature, conflicts) remains fatal. Coordinated update planning, preparation,
suspension, activation restoration and warm-up still refuse a damaged inventory,
including disabled modules. Restoration and warm-up recheck the actual files
rather than relying only on the serialized registry fingerprint. To recover,
deactivate and uninstall the module, then install the same package again, or install a newer version. With
the host stopped:

```sh
./target/debug/talocart --config /private/talocart.toml package-disable --id example
./target/debug/talocart --config /private/talocart.toml package-remove --id example
```

Both hold the exclusive host lease and use the same manager rules: pending work
still blocks deactivating a verifiable module, and removal requires an inactive
entry and preserves private data, schema and settings.
Restore database, media, packages, secrets and compatible binaries together into
new destinations. A private-ledger change may prevent returning to an older
module even if SQL was additive; never delete ledger rows to force rollback.

## Isolated verification

Use `crates/package-fixture` and `scripts/test-package-updates.cjs` for keyless packaging, legacy-envelope integrity, host/package compatibility, transitions and browser controls. Real
external module suites belong with those modules and are excluded from the base
catalog. Test lease conflicts, interrupted migration/intents, replay, tampering,
permission/CSRF failures and unchanged prior executable compatibility. See the
[test guide](guides/testing.md).

## Shared runtime and compatibility review

`talocart-module-sdk` owns bounded command/handshake parsing, replies, loopback
serving and authenticated health/control/request admission. `Hooks` retains the
module-specific pending-work check, shared-request classification and worker
restart notification. SQL migrations, provider clients and business handlers stay
with their owners. Extracting the runtime does not remove embedded base queries
or templates and does not change SDK 6, protocol 3 or manifest 4 on the wire.

The shared SQL surface currently includes all base services reachable from the
embedded web router, not just a module's direct queries. The test checkpoint in
`crates/platform/src/database-contract.json` binds the reviewed migration prefix.
Any migration change fails its unit check until compatibility is explicitly
reviewed: classify additive changes, bump the database contract for incompatible
semantics, and test unchanged supported module artifacts. Updating a snapshot
alone is not proof that a change is compatible. No module minimum checkpoint has
been lowered by the runtime extraction.

The proxy forwards only the validated administrative session cookie. Other
cookies are excluded; duplicate/invalid administrative cookies are not forwarded.
`Content-Disposition` is forwarded for module downloads; private responses still
use `no-store`, bounded buffered bodies and the existing permission checks.

Rebuilt SDK consumers write private diagnostics to `<data_root>/logs/module.DATE.jsonl`.
Read-only `CanStop` probes never initialize diagnostics or write log files.
The directory is mode 0700, records use a bounded asynchronous queue, and seven
daily files are retained. The server operator can read these files directly;
there is no panel log viewer yet. Lifecycle failures identify the last startup
phase; WARN/ERROR tracing events retain severity and static source location but
discard messages, spans and arbitrary field values, including database/provider
errors that could contain secrets. This deliberately limits detail. Queue overflow
can drop events, and SIGKILL cannot guarantee flushing the final records. Existing
historical binaries do not gain this channel until rebuilt.

### Required and optional modules for store availability

Every installed module defaults to `critical = true`, including registries created
before this setting existed. The owner can change this policy on the Modules page;
the POST requires a current owner session and CSRF. It does not change activation,
restart a process or bypass authorization. The policy survives module updates,
host restarts and coordinated backups. Changes are blocked while an application
update is being prepared, and change the registry fingerprint used by update plans.

Ordinary `/health/ready` requires healthy enabled critical modules. An enabled
optional module still runs, is supervised and retried; its failed routes remain
unavailable and its failure stays visible on the Modules page. Database and host
worker health, shutdown and update barriers still apply to store readiness.

`/health/ready?all_modules=true` requires every enabled module, regardless of
criticality. Local and HTTPS updater probes always request this strict mode.
Offline package verification continues to inspect **all installed modules**,
including disabled/optional modules, executables, ledgers and incomplete updates.
Changing a module to optional never turns a failed update into a successful one.

The policy is host registry metadata, not a native manifest/protocol change.
Default true is omitted when writing the registry for legacy compatibility.
An explicit false field requires a host that supports this setting; older hosts
reject it rather than silently dropping the policy. Downgrade planning must account
for that registry compatibility (or restore the corresponding verified backup).
