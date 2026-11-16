# Talocart documentation

These guides cover the standalone, self-hosted Rust e-commerce base: installation,
store administration, development and operations. The interface is available in
English and Italian; repository documentation is maintained in English.

For a first installation, follow the [quick start](../README.md#quick-start) and
[local configuration guide](guides/local-development.md), then review
[commerce settings](guides/commerce.md) before testing a purchase. Contributors
should start with [CONTRIBUTING.md](../CONTRIBUTING.md) and the
[architecture](architecture.md). Read the [security policy](../SECURITY.md) and
[production checklist](operational-readiness-checklist.md) before public operation.

| Topic | Guide |
| --- | --- |
| First start and configuration | [Local development](guides/local-development.md) |
| Products, variants and taxonomy | [Catalog](guides/catalog.md) |
| Taxes, shipping, payments and orders | [Commerce](guides/commerce.md) |
| Customer accounts | [Customers](guides/customers.md) |
| Images and private files | [Media](guides/media.md) |
| Themes, content and shared UI | [Storefront](guides/storefront.md) |
| Responsibilities and data ownership | [Architecture](architecture.md), [boundaries](architecture-boundaries.md) |
| Contributing | [Development](development.md), [test policy](testing-policy.md), [test commands](guides/testing.md) |
| Native extension contracts | [Modules](module-contract.md), [packages](installable-packages.md), [lifecycle](package-lifecycle.md) |
| Extending presentation and modules | [Developer guide](developer-modules-and-themes.md) |
| Installation, backup and recovery | [Operations](release-operations.md), [acceptance checklist](operational-readiness-checklist.md) |
| Release identity and upgrades | [Versioning](guides/versioning.md), [application updates](guides/application-updates.md) |

The [plan](../IMPLEMENTATION_PLAN.md) contains open work. Test reports live under
private `.local/test-runs/`; publication and deployment are separate milestones.
Optional module implementations and private development history are excluded.
