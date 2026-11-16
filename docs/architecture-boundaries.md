# Data ownership and dependency boundaries

[Architecture](architecture.md) owns the overall contract. These are the concrete
responsibilities contributors must preserve.

| Owner | Data and behavior |
| --- | --- |
| Platform identity | Administrators, roles, permissions, sessions, customer identity, confirmation/reset tokens and account authorization |
| Platform database | Connections, migration history/checksums, schema contract and shared advisory lock primitives |
| Platform media | Private source files, media library, references, derivatives, staging/trash and maintenance jobs |
| Platform settings/mail | Persistent infrastructure-independent settings, SMTP encryption/configuration and mail delivery/outbox infrastructure |
| Platform packages | Contract validation, bounded archives, manifests, registry, processes, lifecycle gates and package protocol |
| Platform updates | Authorized local update status/requests; privileged deployment remains external |
| Commerce catalog | Products, variants, options, attributes, categories, brands and public visibility |
| Commerce pricing | Integer amounts, taxes, currency, coupons and quantity discounts |
| Commerce cart/checkout | Cart ownership, authoritative quote, eligible countries/methods and purchase validation |
| Commerce orders | Snapshots, numbering, stock effects, manual/online payment state, fulfillment and commercial outbox entries |
| Commerce content | Public information pages, storefront presentation settings and operation projections |
| Web | HTTP extraction/status, form parsing, typed view models, local rendering and asset delivery |
| App | Startup, command dispatch, configuration and worker composition |

Owners expose services that enforce current permissions; handlers and templates
do not bypass them with queries. Import intake can borrow the caller's transaction
without committing it. External adapters translate their sources and call those
owner APIs instead of writing commercial tables directly.

Shared locks have one owner and a documented acquisition order. Read transactions
do not upgrade to write locks. Preserve SQLSTATE/cause information across service
boundaries and never treat database failures as empty results or hidden products.

Address-book checkout copies preserve technical/access failures and their SQLSTATE;
only actual destination/input rejections become field errors. Cart ownership and
coupon reads address the columns in the supported version-15 baseline directly,
without tolerating missing columns from retired development schemas.

No new crate, trait, cache or queue is justified solely by file size. Introduce a
boundary when it protects a real data/capability contract and has identifiable
consumers. Lexical architecture checks are useful guardrails, not semantic proof.

This repository contains the generic payment/package extension boundaries. Native
provider/importer implementations, their private migrations and tests are external.
Do not copy them into the base to simplify composition or verification.

## Transaction and lock coordination

The shared commercial advisory lock is transaction-scoped and database-wide.
Keep its current protocol until measurements and consumer regressions justify a
change. Different schemas do not isolate it. Inspect current migration functions
and all callers before changing acquisition order, including identity locks,
media publication, foreign keys, annual counters and revision rows.

| Operation | Coordination |
| --- | --- |
| Order confirmation | CSRF, transaction, exclusive commercial lock, current customer authorization, cart, product/variant rows ordered by ID, authoritative quote, snapshot/stock/coupon/outbox, commit |
| Cancellation | Administrative authorization, exclusive commercial lock, order row, stock restoration and outbox in one transaction |
| Online settlement/refund | Atomic claim/lease, provider I/O outside transactions, apply under commercial/order/attempt locks |
| Manual collection | Authorization and order-row lock, receipt and outbox; competes with cancellation on the order row |
| Fulfillment/current status | Authorization and order-row lock; commands do not directly take the commercial lock |
| Catalog and variants | Owner writes under exclusive commercial lock; triggers also cover stock updates, publication, options, combinations, gallery and attributes |
| Tax/currency/countries/methods/coupons | Owner/trigger guards coordinate configuration revisions, active references, quote and order effects |
| Content and languages | Explicit owner locks can also contend with checkout; do not assume content changes are independent |
| Import writer | Delegating actor authorization before commercial lock; caller owns transaction/savepoints/commit and the owner APIs perform writes |
| Setup/demo | Commerce coordinates owner writes and shared fiscal/catalog guards |

Confirmation acquires the commercial lock before customer authorization, whereas
cancellation and import writing authorize first. This difference requires analysis
of all lock consumers; it alone does not demonstrate deadlock. Read transactions
must not upgrade shared commercial locks to exclusive ones.

Order helpers for restore, stock movement, reads and outbox borrow the explicit
transaction. Similar SQL across commands does not justify a generic CRUD owner
or hidden commit. Media combines SQL references with filesystem publication and
must preserve uncertain-outcome handling.

Confirmation checks cart ownership once before replay lookup; this includes its
identity, token and expiry. Public service boundaries continue to authorize their
current sessions, including both the attributed customer and the cart's own
session when supplied separately. These checks are not a permissions cache.

Checkout contention regressions wait for an observed blocker via
`pg_blocking_pids`, rather than using elapsed time as proof of contention. They
cover last-unit variants, quote changes with commit/rollback, removed destinations,
borrowed import transactions and rollback of order/stock/number/outbox when a
notification fails. A generic import test does not prove a real importer's lease
and receipt lifecycle; external module consumers must qualify that separately.

## Internal responsibility boundaries

Request extensions hold `OptionalCustomer`, `AuthenticatedCustomer` and
`CartContext`; services still recheck current sessions and permissions. Remaining
task-local context is presentation/language only. `web::http_policy::private`
provides shared no-store/noindex policy for orders, checkout and customer accounts;
callers preserve status/body and other area-specific headers.

Primary and localized storefront routes share `routing::public_routes`. Their
route group installs `PublicRoute` before the language/customer middleware;
request capabilities come from actual routing, not a second URL-shape list.
URL-shape helpers retained for banner destinations are presentation only.
HTTPS with secure cookies delegates per-client authentication limits to the
configured proxy; services retain session/account limits and ignore forwarded IPs.

`web::variants::options` owns option-editor handlers/view models. Public order
models/errors/APIs stay in `commerce::orders`, with private confirmation, read,
cancellation, stock and notices services. Each operation retains its own complete
transaction and lock ordering.

`platform::mail` owns encrypted transport settings, authorization and validation;
web renders typed forms, and app loads the private key and composes the worker.
Sender/outbox settings remain separate. Recovery owns the key alongside state.

The updater coordinator delegates pure plan validation to `update_plan.py`,
private plan/watermark/journal persistence to `update_state.py`, bounded CLI/cgroup
I/O to `update_host.py` and maintenance/HTTPS probes to `update_probe.py`.
Packaging embeds these sources into the coordinator; runtime bundles retain their
reviewed allowlist. Recovery in the launcher remains independent of that engine.
If systemd collects a transient CLI unit during shutdown, recheck `LoadState`:
remove its operation marker only after confirmed absence; loaded/unreadable state
preserves recovery.
