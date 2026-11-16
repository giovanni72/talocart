# Catalog

[Index](../README.md). Catalog services own commercial data and enforce current
permissions; handlers/templates do not query or authorize on their own.

## Products and variants

Products can be simple or configured by option axes/values and variants. Keep
model/SKU and slug uniqueness across the relevant product/variant namespaces.
Product publication, active category ancestry, availability and configuration
determine public visibility. The administrative public link uses that same
authoritative visibility projection; database failure is an error, not invisibility.

Prices use integer currency minor units with the store's current tax-entry mode.
Tax classes/rates and currency are server validated. Stock, minimum quantities
and pack multiples are enforced during cart updates and confirmation. Browser
values never authorize a price, discount or quantity.

For configured products, the product editor replaces the parent's price and stock
inputs with a read-only summary of active variants: one price or a minimum–maximum
range, and total stock. With no active variants it explicitly says so. The stock
total is informational; it cannot make an individual combination purchasable.
A native **Manage variants** link opens their editor. Simple products retain
editable price and stock. Minimum quantity, pack multiples and tax class remain
shared product settings, including for products with a single variant.

Configured-parent saves preserve the stored parent price and stock under the
commercial lock, ignoring omitted, stale or forged commercial fields. These
values remain available internally (including the new-variant price suggestion);
the summary is never persisted into them. Removing all option axes returns a
configured product to draft in the same transaction, preserving its parent price
and stock for review before explicit republication. Saving an already-simple
configuration does not change publication status. No catalog data migration is needed.

Option axes describe possible configurations; only active valid variants are
purchasable. A missing combination is unavailable. Variant images can override
the product image with product fallback. Preserve existing variant data when
changing axes and handle collisions/invalid configurations explicitly.

## Taxonomy and content

Categories form a hierarchy with one primary category per product and additional
assignments where supported. Brands, attributes and options have distinct roles.
Activation, ancestry and filters affect the public catalog; administrative bulk
actions apply current permissions and server validation to the selected resources.
Lists are paginated and bounded; client selection is not authority.

The store supports explicit content translations and a configured primary-language
fallback. Preserve original user/imported text; changing interface language does
not translate it. Descriptions/information pages distinguish literal text from
versioned rich HTML, with server sanitization. See [storefront](storefront.md).

## Images and checkout integration

Use the private shared [media library](media.md) rather than remote hotlinks or
filesystem paths in templates. Gallery order, product/variant references and
responsive derivatives belong to the owning services. Do not expose storage.

Tax, stock and eligibility are recalculated in checkout. Confirmation creates
the immutable order snapshot and applies stock exactly once in a transaction.
Subsequent catalog edits do not rewrite previous order contents. Cancellation
and payment/fulfillment operations follow their own transitions.

## Extension intake

Generic import intake APIs accept normalized data through commerce owners,
borrowing the caller's transaction without committing it. Source-specific clients,
mapping rules, credentials, jobs and private ledgers belong to external adapters.
This repository includes no provider-specific importer implementation.

## Verification

Select catalog/product/variant/taxonomy PostgreSQL groups and affected consumers
with the [runner](testing.md). Use real database constraints and concurrency for
stock/uniqueness changes. Visible editor/list changes also need EN/IT browser
profiles, keyboard/mobile and native fallback where applicable, plus visual review.

## Related products

The `related-products` component is included in the base. Open **Related products**
in the existing product editor, immediately after **Options and variants**.
The `#panel-related` tab saves automatic suggestion settings atomically with the
main **Save product** action, including when submitted from another tab. Validation
errors retain the submitted setting. Native search and individual add/remove
actions return to the same tab and
preserve the search. Previous `/related` links redirect to this tab. Candidate results stay empty until a nonblank name/model search is submitted.
Search by name/model and add up to 12 directed
links; linking A to B does not link B to A. Remove links individually. Each link
has a bounded display slot; additions occupy the first free slot. Reads require
ProductsRead; mutations revalidate ProductsWrite and CSRF inside a transaction.

Each product can opt into automatic suggestions from its exact category, its
brand, or either. The default is disabled. Automatic suggestions apply only when
no explicit links exist; hidden explicit targets do not activate the fallback.
Suggestions exclude the source and use the public catalog visibility, localized
content, price and image projections. Null categories/brands never match each other.
Results are deduplicated, ordered by name/ID and limited to 4 on product pages, for both manual and automatic recommendations; explicit links use
slot order. Both storefront themes reuse the shared cards and omit empty sections.

The additive `0003_related_products.sql` migration preserves existing products.
Deleting either endpoint removes its links. No automatic reciprocal links or
changes to checkout, stock, imports or prices are introduced.

## Stock control

**Manage stock** in the product's **Price and availability** section defaults to
on, including for imported products. Disabling it applies to the simple product
or to all its variants: published, otherwise eligible products and active variant
combinations remain available independently of recorded quantities. Minimum
purchase quantities, pack multiples, the 999999-piece line limit and all other
publication, price and checkout rules still apply. Inactive variants cannot be bought.

The editor, product list and variant screens show **Stock not managed**. Quantity
inputs are disabled while the policy is saved as off; native forms also preserve
recorded quantities. Saving other fields or disabling control never zeros existing
stock. New unmanaged products/variants start with recorded stock zero. Importers
must preserve this local policy; normal intake defaults to managed stock.

Catalog cards, related cards, available-only filters, product forms, customer and
guest carts and final confirmation all use the current policy. Unmanaged sales
create no stock movements. Cancellation/refund restoration uses immutable debit
records from confirmation, not today's product policy: old managed orders still
restore after control is disabled, and unmanaged orders never restore stock even
if control has since been enabled. Existing idempotency and transaction locks apply.
Check recorded quantities before enabling control again. Stock/policy changes
invalidate stale product editing forms through the existing stock revision.

The initial schema defines stock management as enabled by default. This schema
baseline requires a fresh installation; upgrading an earlier baseline is unsupported.
The `variant-commerce` browser group covers the toggle in EN/IT, both themes,
desktop/mobile and JS/no-JS, without screenshots.
