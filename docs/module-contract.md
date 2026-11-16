# Module contract and source boundaries

The base is MIT-licensed and builds independently:

```sh
cargo build --locked -p talocart
cargo test --workspace --locked
```

The base source tree contains no separately distributed payment-provider/importer
implementation, private module migration, template, asset or module-specific
integration suite. External modules are acquired/built independently and installed
as `.talopkg` native packages. Generic interfaces and synthetic fixtures
remain part of the base so contributors can implement and verify extensions.

## Ownership

The host owns current identity, authorization, sessions/CSRF, shared commerce,
media, error presentation and extension lifecycle. Modules own source/provider
clients, credentials, their specific rules/presentation, private jobs and schema.
They call owner services for commercial changes rather than writing catalog/order
tables directly. Templates use typed data and shared components with EN/IT catalogs.

An import intake may borrow a caller's transaction; the caller coordinates commit
with private receipts/leases. Provider operations use the generic gateway contract:
commerce owns idempotency/payment state and no external call runs in an open SQL
transaction. Do not add an optional implementation as a base runtime, build or test dependency.

## Compatibility and licensing

The base owns `migrations/` and `_sqlx_migrations`. External modules own separate
private ledgers and must validate the host's declared migration checkpoint before
installing or serving. Base doctor/serve validate, never migrate or repair ledgers.
Compatibility is determined by SDK, protocol, database history and target platform,
not the application release number. See [installable packages](installable-packages.md).
SDK 6 requires a token-authenticated, non-mutating health endpoint independent of
request gates. Older SDK-major executables must be rebuilt; no database migration
is introduced by this process contract.

The initial schema uses `online_checkout` and `online_payment_events`.
Provider IDs are opaque and ownership is immutable. Database contract 4 includes
administrative order revisions in the initial schema and requires a fresh
installation; see [architecture](architecture.md).

Each external package must declare owner, license, prerequisites, compatibility,
configuration, private state and test strategy. The base's MIT license does not
automatically license an independently distributed module. Generic interfaces
remain MIT. Previously granted licenses are not revoked by later separation.

## Public source exports

`scripts/release/base-files.json` is the reviewed public file allowlist. Unknown
working files, private notes, archives, runtime/configuration secrets and optional
modules do not enter a base snapshot. Exports contain no Git metadata/history.

```sh
python3 scripts/release/source.py --profile base --output /tmp/talocart-base-source
python3 scripts/release/source.py --check /tmp/talocart-base-source --no-history
```

Choose a new output path; the command refuses existing destinations. It uses the
current reviewed file contents, regenerates the lockfile offline while retaining
registry versions. Rust sources and tests are copied byte for byte, without
composition stripping or formatting during export. A local Cargo index
is required. The working repository and databases are unchanged.

Initialize a **new** Git repository in the reviewed snapshot when publishing it;
never change a private repository's visibility or reuse its `.git` directory.
Use only a fresh initial commit and explicitly chosen public remote. Do not copy
private branches, tags, releases or commit messages. Publication needs its own
explicit action; generating/checking a snapshot does not push or publish anything.

The public allowlist maps paths to themselves so exports can also be regenerated
from a standalone public checkout without access to the private development tree.
