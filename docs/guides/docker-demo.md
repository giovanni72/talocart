# Local Docker demo

Run these commands from a standalone base checkout with Docker Engine/Desktop
and Docker Compose v2. This is a local evaluation environment, not production
packaging. It builds the pinned Rust toolchain in a Linux container; Rust, Node
and PostgreSQL are not required on the host. The first build may take several
minutes and needs network access and sufficient memory for two compiler jobs.
No prebuilt Talocart image is downloaded and no two-minute first-build time is promised.

```sh
docker compose up --build -d
docker compose ps -a
docker compose logs migrate app proxy
```

The migration service must exit successfully, the app become healthy and the
proxy start. Open http://127.0.0.1:3300/setup, choose languages, create the owner
(12–128 character password) and optionally select **Install demo data**.
Setup closes after completion. There are no default administrator credentials.
Visit `/admin` and `/`; the primary storefront language uses `/`, while a
secondary language uses its prefix (for example `/it`).
Demo content uses synthetic illustrations, not the README screenshot catalog.
Review purchasing countries, taxes, shipping and payments before testing checkout.

## Isolation and persistence

Only host loopback port 3300 is exposed; PostgreSQL has no host port. The app
retains its loopback listener; Nginx shares its network namespace and forwards
the published port. Local HTTP uses non-secure cookies. Do not expose this stack
through a public proxy or use real customer data. The fixed PostgreSQL password
is only for this isolated demo database, never a production credential.

The named `postgres-data` and `app-data` volumes preserve the database, media,
SMTP encryption key, logs and package state. No private modules, host packages,
application storage or development database are mounted. The application runs
as a non-root user. The image is a debug evaluation build, not a signed release.
The Dockerfile rejects a Full workspace as build context. Full maintainers must
export a standalone base snapshot before running it.

The separate `migrate` service explicitly runs migrations before `serve` starts;
serve/health checks do not migrate. Re-running Compose preserves existing data.
Rebuild after changing embedded source files. This convenience stack does not
provide production upgrade, backup or restore orchestration.

## Stop, restart and reset

```sh
docker compose down
docker compose up -d
```

`down` preserves both volumes. To permanently delete **this demo's database and
stored files** and return to setup, explicitly run:

```sh
docker compose down --volumes
docker compose up -d
```

Port 3300 must be free. To change it, keep the host port in `docker-compose.yml`,
the app listener/health check and Nginx upstream port in sync so generated
canonical URLs remain correct. Rebuild after changing the bundled configuration.
