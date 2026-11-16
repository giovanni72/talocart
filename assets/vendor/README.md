# Local frontend dependencies

## HTMX

Version 2.0.11. License: `HTMX-LICENSE` in this directory.
Source: https://unpkg.com/htmx.org@2.0.11/dist/htmx.min.js
SHA-256: `d6fdc75f204e6bdefa99b69bf1e6d4ac69b8a364f77929f45c13476b4000f717`.

Embedded and served locally, never downloaded from a CDN by the browser. On
upgrade review origin/license/checksum and update paths, templates and tests.

## Tiptap

Tiptap OSS 3.32.0, MIT. Entry source: `assets/admin/rich-text.js`; versions are
locked in package.json/package-lock.json. Regenerate using `npm run build:editor`
and verify with `npm run build:editor -- --check`. `tiptap/LICENSES.txt` retains
copyright/licenses for all 33 bundled packages, also embedded as bundle comments
so browser recipients receive notices. Packaging retains this directory.
Origin: https://github.com/ueberdosis/tiptap and the locked packages' npm registry.

## Lucide

See [Lucide notices and source checksums](lucide/README.md). Retain ISC and
Feather-derived MIT notices with the selected icons.
