# Development demo content

Demo packages are versioned synthetic optional data embedded in the executable.
The current `catalog-demo` v5 manifest is [catalog.toml](catalog-demo/catalog.toml).
Automatic tests use their own isolated schemas/media, never a shop database.

Initial setup offers an unchecked Install demo data option. Owner/settings/catalog/
receipt commit together; failure leaves database setup incomplete and retryable.
On an already installed/migrated development store:

```sh
./target/debug/talocart --config talocart.toml seed-demo --development --dry-run
./target/debug/talocart --config talocart.toml seed-demo --development
```

Dry-run writes no files/records/sequences. Repeating installation detects an
existing receipt and preserves edits/deletions; it is not a reset/repair command.
No demo administrators, default passwords, customers, orders or payments are
created. Store branding/preferences/default tax class remain unchanged. Synthetic
EUR prices obey the current tax-entry mode; sample rates are not fiscal advice.

## Content

| Resource | Count |
| --- | --- |
| Categories | 4, including a child and an inactive category |
| Brands | 2 |
| Tax rates/classes | 3 each, synthetic 22%, 10%, 0% examples |
| Options/values | 2 / 6 |
| Products | 30 |
| Variants | 4 |
| PNG illustrations | 6, with original SVG sources |

Twenty-seven products are public; draft/inactive-category/unavailable-configuration
examples remain administrative. Variants cover available, zero-stock and inactive
combinations with product-image fallback. Product names/SKUs/slugs are example
content and intentionally retain their original language.

The seeder accepts only a receipt matching the current bundled version. Other
versions are rejected without modifying existing data; there is no expansion or
conversion command. CLI counts describe the whole manifest, not current inventory
after edits. Filesystem publication preceding a later SQL rollback can leave
unreferenced files for the normal 24-hour orphan cleanup; referenced files are
preserved.

## Assets

SVG illustrations are original repository sources; PNG derivatives are embedded
and published through MediaStore, without remote image downloads. To regenerate
in development after npm/Playwright setup:

```sh
node scripts/build-demo-assets.cjs
# Or an installed Chrome:
TALOCART_BROWSER_CHANNEL=chrome node scripts/build-demo-assets.cjs
```

Running the shop requires no renderer/Node. Manifest changes require explicit
versioning, rebuild and relevant tests. Migration/serve do not install demo data.
