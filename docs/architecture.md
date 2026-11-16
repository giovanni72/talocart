# Architecture

Talocart is a modular monolith. The base runs against PostgreSQL and binds to
loopback; an HTTPS reverse proxy owns public network exposure. See the
[ownership map](architecture-boundaries.md) and [open work](../IMPLEMENTATION_PLAN.md).

## Composition and boundaries

| Component | Owns |
| --- | --- |
| `apps/talocart` | CLI, configuration, composition and supervised workers |
| `crates/platform` | Database infrastructure, identity, media, settings, email, package/update contracts |
| `crates/commerce` | Catalog, pricing/taxes, carts, checkout, orders and content |
| `crates/web` | HTTP, middleware, localization, typed view models and Askama templates |
| `crates/module-sdk` | Shared native-process handshake, serving, admission and private diagnostics |
| `crates/package-fixture` | Synthetic package executables for contract verification |

Commerce depends on platform; web depends on commerce/platform; app composes
them. Platform/commerce do not depend on web or optional module implementations.
Optional implementations are also forbidden in base development dependencies.
Tests against real external implementations belong to their distribution workspace.
Handlers delegate authorization and rules to services. Data owners execute
parameterized queries; templates receive typed data and never query.

Askama is allowed in platform only for customer registration confirmation in
`identity/customer_registration.rs`, because identity owns tokens and atomic
outbox insertion. This scoped exception does not permit general HTTP/UI
dependencies there. Architecture checks supplement, not replace, data-flow review.

## Data, transactions and errors

PostgreSQL constraints and transactions protect invariants. Values are bound,
dynamic identifiers come from allowlists and lists are limited/paginated. The
base owns `migrations/` and `_sqlx_migrations`; native packages own separate
ledgers. Migration is explicit: doctor, readiness and serve never alter schema
or repair checksums. The migration command does not backfill order projections: checkout
writes the customer summary in its transaction and the initial schema installs the
current-status trigger. Read-only checks reject missing projections.

Prices use integer minor units; tax rates use basis points with explicit rounding
and overflow handling. Checkout rechecks prices, stock and permissions on the
server. Confirmation persists the validated quote's method/tax details in the
order snapshot, not a previous browser quote. Stock and idempotency are atomic.
Order, payment and fulfillment states remain distinct. Administrative drafts publish
atomic, audited commercial revisions only for eligible unpaid/unshipped orders;
see the [commerce contract](guides/commerce.md#administrative-order-creation-and-revision).

`platform::database::commercial_lock` centralizes the advisory lock contract.
The global commercial lock preserves trigger lock ordering; reducing it requires
an explicit concurrency increment and measured evidence. A local benchmark is
not a production capacity guarantee. Settings reads use one consistent
transaction and current identity/session locks; no permissions cache authorizes
an operation. Do not hold synchronous guards across await.

Recoverable errors use `Result` and typed domain/access/technical causes.
Technical errors preserve validated SQLSTATE without leaking SQL or credentials.
Transaction retry is limited to serialization/deadlock SQLSTATEs `40001`/`40P01`,
with rollback and at most three attempts. Validation, authorization and connection
failures are not automatically retried. External providers never run inside SQL
transactions.

## Identity and presentation

Admin and customer sessions are separate and persisted in PostgreSQL. Revocation,
expiry, ownership, permission and CSRF checks apply to ordinary and HTMX requests.
No GET mutation is allowed. Password hashing and blocking work run with bounded
concurrency. Customer registration can be immediate or require a one-use expiring
email token, atomic outbox entry and rate-limited resend, without link auto-login.

Askama escaping and server HTML sanitization protect presentation. Rich text is
explicitly marked with `<!--talocart-rich-text:v1-->`; unmarked plain text remains
literal. HTMX enhances native forms; pages/fragments share security and validation.
English/Italian catalogs, semantic HTML, focus, keyboard, mobile and appropriate
no-JavaScript fallback are shared UI contracts. User content retains its language.
Static and dynamic system labels use Fluent IDs directly; typed errors are
localized at the HTTP boundary. Templates render localized values without a
second translation pass. There is no general Italian-string lookup dictionary.
The fixed discriminators from the published media_usage SQL view have a dedicated
presentation mapping; user content and stored diagnostic payloads are not rewritten.

Themes `default`/`essential` are compiled and selected from an allowlist. Template,
asset, language and demo changes require rebuilding. Infrastructure configuration
is read at start. SMTP transport is persisted through platform mail services;
its password is authenticated-encrypted using a private key in
`<media-root>.secrets`, separate from the database and included in coordinated backup.

Embedded asset routes prepare their content-derived SHA-256 validators once at
router composition. GET, HEAD and 304 reuse the same ETag without per-request
hashing or database access. URLs and mandatory cache revalidation stay unchanged.

## Payments and native extensions

The base exposes bank transfer and cash on delivery. Generic gateway types live
in platform; commerce owns online attempt leases, idempotency, captures/refunds,
snapshots and outbox. Provider clients, credentials, signature verification and
provider-specific UI/assets belong to external packages. This repository includes
no implementations of those packages.

The validated manifest restricts package identity, routes, platform, private ledger
name and capabilities. The host validates authorization and CSRF before delegation.
Payment RPC uses authenticated loopback, typed bounded messages, timeouts and
gates preventing deactivation during confirmation/commit. Provider results are
normalized; external calls happen outside commercial transactions. Package
execution is trusted code, not a sandbox. See [contracts](installable-packages.md).

Three terms stay distinct. The compiled `base` inventory (`platform::modules`)
lists non-switchable internal areas; it has no activation state. `WebExtension`
(`web::extensions`) is the route/menu composition API a module process uses to
build its own router on the shared SDK; the production host composes none, and
only the test-only integration workspace composes modules statically. Installable
native packages are separate `.talopkg` executables that the host verifies,
starts, supervises and reaches through its authenticated proxy and payment RPC.

Online methods use the common `online_checkout` implementation and a unique,
immutable `provider_id` per package. Multiple providers may coexist; checkout,
webhooks, presentation and reconciliation select the recorded identity. New order
snapshots and all attempts persist their immutable provider identity. Session/payment/refund/event IDs are opaque and unique within
a provider. Events are stored in `online_payment_events` with provider-scoped IDs.
Webhook references identify session, payment or refund explicitly, so equal IDs
in different object namespaces remain unambiguous. Conflicting references are
rejected. A replacement provider cannot process or refund another provider's work.

The module normalizes authoritative payment/capture/refund results. Its validated
manifest declares webhook headers and the guaranteed idempotent retry window;
provider-specific ID rules and currency amount bounds belong to the adapter.
The manifest exposes neutral amount bounds, captured outside transactions and
checked against final checkout totals before order/stock writes. SDK 6.0,
payment RPC 2 and database contract 4 define this generation. SQL column names are storage details, not provider implementation dependencies.


The initial schema includes administrative drafts and order revisions. Follow-up
migrations add guest policy and related products.
Database contract 4 requires current readers to select active order lines; prior
versions remain archived with immutable stock moves and an operator audit.
The revised initial schema requires a fresh installation; previous schema
checksums are intentionally incompatible. Attempts, captures, refunds and
events always name their provider explicitly.

## Operations

Native modules default to required for store readiness; an owner may mark a module
optional. Update probes explicitly require all enabled modules, and offline
verification still checks every installed package.

Workers persist email/image/payment work and are supervised; unexpected exit
makes readiness fail and terminates the host. The private ERROR journal uses
bounded writes on a dedicated thread, daily UTC rotation and owner-only reading.
Console diagnostics remain separate from CLI stdout results.

Manual storefront maintenance uses persistent settings with permission/CSRF,
returning the shared no-cache 503 page while preserving administration, health
and required assets. Search indexing is an explicit persisted opt-in, off by
default; only successful allowlisted public GET/HEAD responses may opt in,
including coherent 304 cache transitions. Private pages always retain exclusions.

The OS updater is separate from HTTP. Root-owned verification, signed manifests,
coordinated backup, writer fencing, restricted application-user CLI execution,
read-only verification and recovery guard its lifecycle. `serve-verify` starts
no workers or write sessions and exposes restricted public probes only. Installation
and updater code require final Linux/systemd qualification; source publication
does not attest that qualification. See [updates](guides/application-updates.md).
