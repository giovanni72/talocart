# Media and private storage

[Index](../README.md). MediaStore owns sources, references, derivatives, staging,
trash and maintenance. Storage is private and is not a public web root.

## Library and references

Products, galleries, variants, branding and accounts reference owned media through
services. The shared library provides folders, selection, file management and
trash with current permissions/ownership. Client filenames/paths never authorize
filesystem access. Private files and avatars retain their access checks.

Upload/acquisition validates supported content, sizes and safe paths. Image
processing and filesystem work are isolated with bounded blocking concurrency.
Responsive copies are derived caches; source files are the recovery authority.
Template image sizes must match theme layout/density and breakpoints.

Storefront `/media/image/{id}/{profile}` URLs serve the selected existing file
directly, including GET/HEAD and conditional 304 responses. Resolve current public
source access and the available representation before matching its object ETag.
Keep `public, max-age=0, must-revalidate`: a profile may select a new cache revision,
and publication may be revoked without changing the URL. This avoids the former
profile-to-object redirect without relaxing publication checks or generating files
in HTTP requests. Exact/legacy object URLs retain their repair redirects.

Brand menu/detail badges use 128px thumbnails; product cards use thumbnails with
a 512px candidate for high-density displays, using density descriptors derived
from the source dimensions and profile caps. Small sources retain their native size.
Brand directories retain larger copies. All profiles preserve proportions and never
upscale stored pixels.

Product photo links carry responsive candidates as data, without preloading every
large image. JavaScript changes only the selected image and current-photo marker,
preserving live purchase/variant forms, keyboard focus and quantity. Local
Back/Forward entries and language links follow the selected photo. Native links
and reloads use the same server-validated photo/variant selection and dimensions.

Never remove a referenced file to fix an application error. Missing sources or
derivatives require diagnosis and explicit repair. Unreferenced staged/orphaned
files follow the common retention/cleanup rules rather than immediate unsafe
deletion after a transaction rollback. Preserve user/imported filenames/content
where compatible with safe storage naming and collision handling.

## Maintenance

Background discovery and derivative jobs are supervised and persisted. HTTP
requests do not scan arbitrary source directories or run unbounded image work.
Inspect available explicit administration commands with `talocart --help`:
`media-variants`, `media-acquire`, `media-cache` and `media-prune` have distinct
repair/cleanup roles. Use preview/dry-run where supported before mutation.

`media.root` is persistent; relative paths resolve against the TOML directory.
Keep packages/logs/secrets separate and private. The SMTP encryption key resides
in `<media-root>.secrets`, not in a public media route.

## Backup and verification

Back up database, media, packages, configuration, encryption secrets and compatible
executable together while all writers are fenced. A database dump alone is not a
complete backup. Restore only into new paths/database and verify references,
source hashes, private modes and operational readiness; see [operations](../release-operations.md).

Media changes need real filesystem/database tests for references, failure cleanup
and concurrency. Library interactions require browser keyboard/focus, EN/IT,
mobile/desktop and native fallback profiles plus human visual review. Use only
synthetic files and dedicated storage in automatic tests.
