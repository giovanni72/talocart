# Customers and accounts

[Index](../README.md). Customer sessions are separate from administrator sessions.
The base supports guest checkout, accounts, profile/address/tax details,
persistent carts and order history. Ownership/permissions are checked on the server.

## Guest checkout policy

In **Settings → Customer fields → Customer registration**, **Guest checkout**
allows purchases without an account (the default, including upgrades) or requires
an account before checkout. The setting uses the same permission, CSRF and
revision checks as registration activation, but remains independent of email
confirmation.

When guests are disabled, new checkout steps redirect anonymous customers to
sign in or register, retaining the cart and returning to checkout after login or
immediate registration. Email-confirmation mode still requires activation and a
subsequent login. Server-side checkout services recheck the policy in their
transaction, including final order creation. Entering an existing customer's
email is not authentication. Browsing and cart editing remain public. Existing
orders, receipts, payments and idempotent order replays remain accessible to their
original owner after a policy change.

## Registration and email confirmation

The store can activate registration immediately or require email confirmation.
Confirmation creates a pending inactive account, one-use expiring token digest
and outbox entry atomically. Resends are rate limited. The confirmation link
does not automatically sign the customer in. Valid mail transport/sender settings
are required for confirmation mode; failures remain explicit rather than silently
activating the account.

Changing account details and resending confirmation use native POST forms with
CSRF, including HTMX. Superseded pending messages/tokens are handled transactionally;
technical email/database failures roll back account/token/outbox changes and
preserve their cause. Never expose token values or account details in diagnostics.

## Login, profile and purchases

Sessions have persisted expiry/revocation and secure-cookie settings consistent
with the configured origin. Rate limits apply, conservatively sharing loopback
proxy addresses. Logout invalidates the relevant session. Browser ownership is
not inferred from task-local presentation state or user-supplied account IDs.

Profile, addresses, fiscal details and configured customer fields are validated
by services. Customer groups may affect quantity discounts with server-authoritative
eligibility. Guest and authenticated cart transitions preserve ownership and
revalidate quantities, prices, stock and method selection. Existing order snapshots
remain immutable when profiles or catalog records change.

Checkout starts from the existing cart draft, or from profile contacts and permanent
billing data when no draft exists, then fills delivery details and phone from the
customer’s default address, if set. Loading the page does not persist these values. The profile phone and address-book phone are
separate values; saving an address does not update the profile phone. Signed-in
customers with saved addresses can select one directly in checkout and submit
**Use address**, including without JavaScript. This fills delivery fields and the
contact phone while retaining submitted contact names, email, delivery notes and
independent billing/fiscal fields. Selection only fills the form; **Continue**
validates and saves it. Address ownership is rechecked on the server, and the
normal checkout validation still enforces current destination/field policies.
Initial profile prefill, **Use address** and the native **Update fields** action
do not mark incomplete fields as errors. **Continue** validates the submitted
form. Returning to an already saved checkout draft still highlights fields that
no longer satisfy the current store requirements.
Pressing Enter in a checkout text field submits **Continue**, including without
JavaScript; applying a saved address requires activating **Use address** explicitly.
Existing drafts are never silently overwritten by changes to the address book.

The development baseline defines this flag and index in the initial schema; older
development databases need an explicit preserving alignment before running it.
No historical upgrade path is provided for that rewritten baseline.

Customer and administrator address editors expose **Use as default address**.
`customer_addresses.is_default` is false unless explicitly selected; even a sole
address is not automatically made default. A partial unique index permits at most
one default per customer. Saving another default atomically clears the previous
flag and increments that address’s revision, invalidating stale forms. Unchecking
or deleting the default leaves no default; another address is not chosen implicitly.
The address book lists the default first and labels it in account/admin lists and
the checkout selector. Independent billing data and immutable orders are preserved.

## Assisted recovery

After independently verifying the account holder, an authorized operator can
issue a one-use reset token into a new private file:

```sh
./target/debug/talocart --config talocart.toml customer-reset-token --email customer@example.test --output /private/new-customer-token.txt
```

Use it at `/account/reset` under the store's active language prefix. The token
lasts 30 minutes and does not change the password until consumed. Administrator
recovery uses the separate `reset-token` command and `/admin/reset`. Never put
tokens in public issues, URLs shared with others or ordinary logs.

## Verification

Account changes require relevant identity/customer PostgreSQL and HTTP coverage:
expiry, revocation, token reuse, ownership, CSRF, failure rollback and concurrent
conditions. Visible forms also require EN/IT, mobile/desktop, keyboard/focus and
appropriate no-JavaScript profiles, using synthetic customers and captured mail.
