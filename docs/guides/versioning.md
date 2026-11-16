# Versioning and build identity

[Index](../README.md). The only application version is
`[workspace.package].version` in `Cargo.toml`, inherited by workspace packages
and aligned with `Cargo.lock`. It is independent of SDK/protocol/database versions.

```sh
python3 scripts/release/version.py check
python3 scripts/release/version.py prepare X.Y.Z
```

Replace X.Y.Z with the selected SemVer. Prepare updates manifest/lockfile together;
it does not commit, tag, publish or deploy. Inspect the diff before committing.
If a version newer than the last published tag is already prepared, use it rather
than bumping twice. Application changes do not automatically bump SDK/protocol/DB.

An official release requires clean final main, fetched/rebased as appropriate,
with matching annotated tag `vX.Y.Z`:

```sh
python3 scripts/release/version.py tag
cargo build --release --locked -p talocart
./target/release/talocart build-info
```

Never move a published tag. Build after the tag so embedded identity records it.
The tool does not push. CLI `--version`/`build-info`, administration and release
manifest identify the version/commit/dirty/tagged state. Git is used at build
time only; source archives without Git build with an unknown development identity.

Ordinary debug fingerprints executable inputs; documentation/test-only changes
do not force an application rebuild. Cargo still compiles changed tests. Release
and `TALOCART_BUILD_IDENTITY_EXACT=1` use exact full-checkout identity for packaging.
Do not use an old binary or resume fingerprints as proof of compilation.

Packages verify executable version/commit/source fingerprint, pinned compiler,
target and composition. `--release` rejects dirty/untagged builds. Compile from
the final tag, qualify final artifacts once and promote those bytes rather than
rebuilding between staging and production. A public initial source snapshot has
its own Git history; private development tags/commits are not imported.
