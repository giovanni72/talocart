# Storefront, content and shared UI

[Index](../README.md). Server-rendered Askama pages provide native forms; HTMX
enhances interactions using the same validation/authorization. User-facing text
must remain available in English and Italian.

## Languages and content

Setup enables English, Italian or both and chooses an enabled primary language.
The primary language uses the root; other languages use configured prefixes.
Explicit content translations fall back to the primary language. User/imported
content retains its original text and is not translated by changing UI language.

Product and information-page descriptions distinguish literal text from rich
HTML with the version marker `<!--talocart-rich-text:v1-->`. Unmarked text remains
literal. Owners normalize values and sanitize allowed HTML on rendering, including
HTML submitted through the editor. Metadata/SEO remains plain text. Tiptap's
local Visual/HTML editor enhances a native textarea; it does not bypass server rules.
Rich HTML preserves table captions, row groups, header/data cells and their
`colspan`, `rowspan` and header `scope` attributes. Styles, event handlers and
scripts are removed. Tables use shared storefront styling and remain within the
content region on mobile. Table cells are separated in plain-text metadata.
Tables outside the visual editor's schema stay in HTML mode so opening or saving
the description cannot flatten them.

## Themes

`default` and `essential` are compiled themes with a persisted allowlisted choice,
palette, local fonts and density. They share commercial services/forms and
permission checks. Templates/assets are embedded and require rebuild; the merchant
does not need Node or Rust to run a binary. Arbitrary template/CSS filesystem
paths are not configuration inputs.

Both themes show the running executable's compiled build display identity in small
text in the shared footer, with an English/Italian label. This is the same identity
as CLI build information; no runtime Git lookup or database version setting is used.

Doctor/serve refuse a persisted theme absent from the build, rather than silently
selecting another. Changing/removing themes requires preserving migrations,
compatible configuration and explicit operator action. HTTP presentation policy
includes language/theme; stale HTMX requests can trigger full-page navigation,
preserving path/filters. Shared form security remains independent of theme.

## Essential UI components

Use `assets/ui.css`, macros in `crates/web/templates/ui.html` and the examples
at `/admin/components`. Reuse `ui-button`, `ui-control`, `ui-check`, `ui-message`,
`ui-empty`, shared pagination/cards and the existing confirmation/state components.
New variants belong in the shared components and examples, not duplicated page CSS.

Preserve semantic elements, associated labels/errors, native form names/values,
HTTP methods, CSRF, keyboard/focus, mobile layout and appropriate no-JavaScript
fallback. Provide EN/IT visible text and accessibility labels in shared catalogs.
Use local icon/font assets with third-party notices. A screenshot does not replace
interaction checks or human visual assessment.

## Manual maintenance

Settings → General includes a standard maintenance checkbox. It is staged and
committed by the shared Save settings action, atomically with the other settings;
unchecking it disables maintenance. Validation errors and stale submissions apply
nothing, and the administration panel remains reachable.

The owner-authorized storefront maintenance setting is persisted with revision,
permission and CSRF checks. The gate returns the same EN/IT no-cache 503 page used
by the updater, before creating customer/cart sessions. Administration, health,
required assets and relevant package callbacks remain accessible. Manual closure
does not change the privileged updater's writer fence or authorize an update.
Verify persistence after restart and re-opening the store.

## Search indexing and cache

Search indexing is an explicit store opt-in, disabled by default. Only successful
allowlisted public GET/HEAD pages and permitted public resources can use
`index, follow`. Private/admin/account/cart/checkout responses stay excluded.
Changes preserve correct 304 cache policy, replacing stale negative directives
when indexing is enabled. Embedded assets/default favicon remain conservative
without database dependencies. `robots.txt` allows fetching noindex directives
and announces the sitemap only when opted in. Maintenance preserves this choice.

Public/private cache policy also separates full pages and fragments. Settings,
authorization and response failures must not silently serve the wrong cache class.
Use real browser cache tests for indexing transitions and real database isolation
to prove embedded asset routes do not wait for settings locks.

## Extending and verifying

Read the [theme developer guide](../developer-modules-and-themes.md) before adding
a compiled theme. Shared components/themes require consumer coverage, often the
full browser matrix. Page-only changes use targeted coverage. Always rebuild
changed embedded inputs before testing and inspect EN/IT mobile/desktop layouts.

## Payment presentation

Receipts use `online-payment-panel` for payment actions. The default theme keeps
its grid/spacing on screen and excludes the panel from print. Provider fragments
use shared `checkout-payment-title` and `checkout-payment-summary` classes; theme
selectors do not depend on private module IDs. Verify computed styles in both
screen and print media.
