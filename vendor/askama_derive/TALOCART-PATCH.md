# Askama configuration parser patch

Source: crates.io `askama_derive` 0.16.1, upstream revision
`9ae4d5bdf4352b35e34106e38944c90d7a7ede3e`.

Backport only the TOML parser replacement from official commit
[3863808](https://github.com/askama-rs/askama/commit/3863808cf594054f7644979ed92caaafc878f378):
use maintained `toml` with `parse` and `serde` features instead of `basic-toml`
(RUSTSEC-2026-0374). The two changed upstream files are `Cargo.toml` (normalized
registry manifest) and `src/config.rs`. All other source files are unchanged.
Registry bookkeeping, the original workspace manifest and the crate lockfile
are omitted; the application lockfile owns dependency resolution.

Keep the upstream MIT/Apache-2.0 license files. Template lookup, configuration,
syntax, escaping and code generation remain unchanged. Do not disable `config`:
shared mail frames, themes and optional modules require their search directories.

Remove this directory, the workspace patch/exclusion and the export mapping
when a compatible published Askama release contains the fix; then run template
consumer tests and audit both workspace and standalone base lockfiles.
