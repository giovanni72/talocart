# Local development and configuration

[Index](../README.md). Public Linux operation and recovery are covered in
[operations](../release-operations.md); use the [test policy](../testing-policy.md)
for verification resources rather than the application database.

## First start

Install the pinned Rust toolchain, native build tools and PostgreSQL 17. Node is
needed for asset development/browser tests, not for running the application.
With a PostgreSQL account allowed to provision a role/database:

```sh
createuser --pwprompt talocart
createdb --owner=talocart talocart
cp talocart.example.toml talocart.toml
chmod 600 talocart.toml
```

Add `url` under `[database]` in the private TOML. Use your environment's
PostgreSQL connection URL, encoding reserved characters in credentials. Never
commit the file or paste the URL into reports/shell history.

```sh
cargo build --locked -p talocart
./target/debug/talocart --config talocart.toml migrate
./target/debug/talocart --config talocart.toml doctor
./target/debug/talocart --config talocart.toml serve
```

Open `/setup` at `http://127.0.0.1:3000`, select store languages and create the
owner. The password must contain 12–128 characters; display name is optional.
Setup is atomic and one-use, without an installation code/default credentials.
Restrict external access to the operator until it is complete. Subsequent starts
use doctor/serve. Ctrl-C/SIGTERM initiate graceful shutdown. Building does not
replace a running process.

## Configuration

Only `talocart.toml` is searched by default. An explicit missing `--config` file
fails. `.env` is not read automatically. Unknown fields are rejected. Supported
`TALOCART_*` overrides take precedence; no old project-name aliases are used.

| Setting | Contract |
| --- | --- |
| `database.url` / `TALOCART_DATABASE_URL` | Private PostgreSQL connection |
| `database.max_connections` / `timeout_seconds` | 1–50 connections; timeout 1–30 seconds |
| `server.listen` / `TALOCART_LISTEN` | Loopback IP, nonzero port; default `127.0.0.1:3000` |
| `server.public_origin` / `TALOCART_PUBLIC_ORIGIN` | Exact public HTTPS origin, paired with secure cookies |
| `server.secure_cookies` | False for local HTTP, true for public HTTPS |
| `media.root` / `TALOCART_MEDIA_ROOT` | Persistent private source/cache storage |
| `packages.root` / `TALOCART_PACKAGES_ROOT` | Private package state, distinct from media |
| `logging.root`, `logging.max_files` | Private daily ERROR journal |
| `updates.root` | Optional absolute OS updater state directory |
| `TALOCART_LOG` | Console diagnostic filter, independent of the ERROR journal |

Relative paths are based on the configuration file's directory, or current
directory without a file. Use absolute service paths. File changes require
restart. SMTP is configured only in Settings → Email. Native packages own their
settings; base configuration does not contain provider credentials. The reserved
legacy static-import flag cannot enable an absent adapter in this distribution.

## Initial store settings and diagnostics

Setup supports English, Italian or both with an enabled primary language.
Review store identity, taxes, purchasing countries, shipping, payments, email
and public origin before testing checkout. See [commerce](commerce.md).

Doctor checks configuration/connection/schema and referenced files without repair;
serve refuses incompatible schema. `/health/live` checks process liveness and
`/health/ready` checks runtime prerequisites. Neither migrates. Supervised worker
failure makes readiness negative and exits the host.

Assisted owner recovery, only after independently verifying the account holder:

```sh
./target/debug/talocart --config talocart.toml reset-token --email owner@example.test --output /private/new-admin-token.txt
```

The output must be a new private file. The one-use token lasts 30 minutes and
is entered at `/admin/reset`; issuing it does not change the password. Customer
recovery is separate; see [customers](customers.md). Never include reset tokens
in issues or ordinary logs.

## Demo and standalone builds

```sh
./target/debug/talocart --config talocart.toml seed-demo --development --dry-run
./target/debug/talocart --config talocart.toml seed-demo --development
cargo build --release --locked -p talocart
./target/release/talocart build-info
```

Demo installation is optional and preserves subsequent edits/deletions. It
creates no credentials/customers/orders/payments; see [demo content](../../seeders/README.md).
The executable embeds templates/assets/migrations. Keep PostgreSQL, configuration
and persistent state outside the versioned executable directory.
