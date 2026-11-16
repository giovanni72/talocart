# Commerce

[Index](../README.md). The base includes bank transfer, cash on delivery and
manual payment recording. External online providers are separately distributed
packages; their clients, credentials, webhook verification and UI are absent here.

## Store configuration

Before an order can be placed, publish a purchasable product, configure its tax
class and stock, enable purchasing countries, activate matching shipping methods
and activate a supported payment method. No countries are enabled initially.
Demo tax rates/content are synthetic examples, not recommended fiscal settings.

The store has a currency and inclusive/exclusive tax-entry mode. Amounts are
integer minor units, rates basis points, rounding explicit and overflow checked.
Existing products constrain changes that would reinterpret their prices. Tax
classes, rates, geographical zones and customer tax details are validated by the
server. Shipping/payment fees and thresholds are part of the authoritative quote.

Coupons and group quantity discounts follow persisted eligibility, validity and
usage rules. Clients cannot submit authoritative discounts. Preserve guest and
authenticated ownership, minimum quantities and pack multiples.

## Checkout and orders

Checkout supports signed-in customers and, when enabled in the
[customer settings](customers.md#guest-checkout-policy), guests. Server validation governs
contacts, fiscal details, addresses, destinations, shipping and payment eligibility.
Native forms remain functional where applicable; HTMX uses the same rules/CSRF.

Confirmation rechecks availability and pricing, uses the validated quote's method
and tax details, writes a commercial snapshot, allocates the order reference and
applies stock atomically. Cart/revision idempotency prevents duplicate orders;
the last item cannot be sold twice. Technical failures preserve causes and roll
back all effects. Only serialization/deadlock failures have bounded transaction retries.

Order, payment and fulfillment states are separate. Cancellation restores stock
once under its allowed transitions. Manual payment recording is authorized and
idempotent. Fulfillment has status, tracking and internal notes without rewriting
the purchase snapshot. Order receipts are not fiscal invoices.

## Administrative order creation and revision

Orders → New order creates a persisted draft. Orders → Drafts lists the 50 most
recently saved open drafts. `orders.write` is required for drafts, previews and
publication; selecting or searching customer accounts also requires
`customers.read`. This is an administrative capability, independent of the guest
checkout policy, and never signs in as a customer or changes their address book.

Select an existing account (copying its profile, first/default delivery address
and saved billing data) or enter contact and address details without creating an
account. Account attribution is fixed at confirmation and cannot be reassigned
when revising an existing order. Drafts can be incomplete and reserve no stock,
allocate no order reference and enqueue no messages. Native forms support product/
variant search, add/remove, quantity changes, field-policy refresh and saving.

The editor groups items, customer details, addresses and shipping/payment methods
in shared tabs. The selected section survives form submissions; Enter in a search
field runs that search. Without JavaScript all sections remain visible. The
confirmation actions stay beside the form on wide screens and follow it on mobile.

Blank unit prices use current catalog prices and the eligible customer/guest
group quantity schedule. Explicit unit prices are final agreed unit prices in
the store's inclusive/exclusive entry mode. The preview identifies differences
from current catalog prices. Existing order prices and shipping/payment entered
amounts are prefilled, so catalog changes do not silently replace them. Clearing
an amount explicitly opts into current pricing. Tax resolution, quantities, stock,
enabled destinations, methods and thresholds are always checked on the server.
Only bank transfer and cash on delivery are supported for administrative creation.
Shipping and payment fees may be overridden in the store's tax entry mode.

Review displays the saved draft, destination, billing, item/tax/charge breakdown
and final total separately from the editable form. Confirmation requires a reason/
source and explicit acknowledgement, then rechecks the draft revision, quotation,
order state and available stock. Changed quotations require another review.
Confirmation is atomic and idempotent by draft ID; replay never allocates another
reference, changes stock twice or sends another message.

Only open, unpaid, unshipped orders using offline methods and without any online
payment attempts may be revised. Concurrent payments, cancellation, fulfillment
and edits cannot overwrite a completed operation. Existing current unit prices
are retained, including previously applied group discounts. An existing coupon's
redeemed terms are retained and reallocated across revised merchandise without
another redemption; adding/replacing a coupon during revision is unsupported.
New-order coupons use current eligibility and usage limits.

Revision retains the same ID/reference and account attribution. Previous lines
are archived (`order_lines.active=false`); immutable debit/credit records preserve
the stock policy of each version. Current reads and cancellation use only active
lines. Stock restoration, replacement debits, totals, customer list projection,
audit and draft publication commit together. Audit snapshots retain product/variant
identity, item descriptions, unit prices, fiscal class/rate/mode, net/tax/gross
amounts and order totals. Before-snapshots identify the archived line records.
Audit history records the operator,
time, reason and before/after commercial snapshots, including prior addresses.
It never rewrites payment receipts, historical lines or stock movements.

Customer email is an explicit checkbox at confirmation or **Send order summary**
on the order detail; delivery requires enabled, valid mail configuration.
New-order staff notifications follow the email configuration. Revision supersedes
unaccepted obsolete summaries, and each current-summary request is idempotent by
order revision. An SMTP message already in flight cannot be recalled; later
messages wait for its attempt/lease to finish. Saved drafts send no notifications.

Draft lists and commercial revision history use bounded pages of 50 entries;
older drafts and audit records remain reachable through native pagination.

Administrative order editing is part of `0001_initial_schema.sql`. This schema
revision targets a fresh installation; existing migration checksums are not
compatible and must not be rewritten in an installed database.
Database contract **4** introduces active versus archived lines and administrative
order sources; old contract-3 modules must be rebuilt/repackaged for the current
host. SDK/payment RPC versions remain independent. Apply migrations explicitly;
`serve` and `doctor` do not apply them.

## Email

Configure transport, credentials, sender, recipients and enabled notifications
in Settings → Email. SMTP is persisted, not read from TOML/environment. Passwords
are authenticated-encrypted; the separate private key is in
`<media-root>.secrets`. Preserve it in coordinated backup/restore. Saved secrets
are not returned to HTML. Changes use permissions, CSRF and revision checks.

Commercial actions enqueue transactional email through the outbox atomically
with their data changes. Workers deliver outside SQL transactions, retaining
retry/lease state. SMTP acceptance is not final delivery. Lost acknowledgements
can create uncertain delivery; Message-ID supports reconciliation, not an
exactly-once guarantee. A suspended/failed queue is not grounds to repeat an order.

Use dedicated Mailpit for development, with the [test environment](testing.md).
Customer email-confirmation registration requires valid enabled mail settings.

## Generic online-payment contract

Commerce owns attempts, leases, idempotency, state, reconciliation, captures/refunds
and outbox. A trusted package implements network operations and returns typed
normalized outcomes; it does not mutate orders or stock directly. External calls
never run in an open commercial transaction. Authenticated loopback RPC, bounded
messages and lifecycle gates protect confirmation/deactivation.

Online methods share the `online_checkout` implementation discriminator, with one
immutable `provider_id` per method. Each installed payment package registers its
own method; merchants configure names, instructions, fees and activation independently.
Checkout filters availability and amount limits for each provider, then holds the
selected provider's lease through confirmation. New order snapshots and attempts
retain immutable provider ownership; IDs and events are scoped to that provider.
Disabling a package hides its method from new checkouts without deleting history
or assigning existing orders to another provider. Shared UI uses neutral
`online-payment-*` keys in EN/IT. Use the [extension contract](../installable-packages.md)
for independently developed packages.

## Verification and scope

Use real PostgreSQL for amounts, constraints, transactions, concurrency and
idempotency; browser tests cover actual checkout interactions and error recovery
in EN/IT, both themes, desktop/mobile and JS/no-JS where applicable. Production
qualification additionally needs final artifacts, failure/restore drills and
the [operational checklist](../operational-readiness-checklist.md).

Returns, partial shipments/refunds, subscriptions and electronic invoicing are
outside the current scope. Define them as separate increments rather than
inferring their behavior from order/payment status fields.
