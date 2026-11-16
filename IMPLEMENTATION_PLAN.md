# Talocart development plan

Open work and completion criteria for the base. Current behavior is documented
in [architecture](docs/architecture.md) and the [guides](docs/README.md).
The application is a development beta; publishing sources is not production approval.

## Before production use

- Qualify an identified candidate using the [test policy](docs/testing-policy.md)
  and [operational checklist](docs/operational-readiness-checklist.md). The first
  production entry requires full local coverage, final Linux artifacts, clean
  installation/upgrade, real restore, failure drills, benchmarks and visual review.
- Verify the actual target environment: HTTPS, access, lifecycle, private storage,
  monitoring and recovery. Local tests do not qualify a server.
- Configure trusted update public keys, release source and the separate updater
  service where needed; verify publication, approval, owner request, fencing and
  recovery with the [update procedure](docs/guides/application-updates.md).
- Replace the [temporary Askama parser backport](vendor/askama_derive/TALOCART-PATCH.md)
  with a compatible published upstream release when available, then remove the
  local patch and rerun template consumers and dependency audits.
- Define store-specific taxes, shipping, payment methods, communication and
  operational responsibilities before launch. External provider readiness belongs
  to each separately distributed package.

## Known verification limit

A historical mobile no-JavaScript coupon test timed out once. Later isolated
runs passed. A redirect-wait race was subsequently reproduced and fixed in the
test helper, with a deterministic regression. The original timeout did not retain
enough page evidence to prove that race caused it. Do not declare the historical
cause resolved or mask intermittent failures with retries/timeouts. Current
diagnostics and reproduction are in the [test guide](docs/guides/testing.md).

## Future scope

Multi-store, marketplace, electronic invoicing, advanced B2B, returns, partial
shipments/refunds and subscriptions need separate scope and acceptance criteria.
They are not automatic prerequisites for publishing the base.

Keep completed implementation detail out of this plan. Preserve open issues here
and current contracts in their owning guides; do not add private history pointers
that do not exist in this repository.
