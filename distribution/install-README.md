# Talocart first installation

This independently root-signed archive installs the base and updater together.
Verify its detached signature with an independently trusted root public key
**before** extraction. Extract into a root-owned directory with no writable
ancestors. The normal application archive is not authorized root code.

Provision Ubuntu 24.04, PostgreSQL 17 role/empty database, application account,
private TOML, TLS, Nginx and trusted keys first. Keep access operator-only until
`/setup` creates the owner. Inventory/fence all writers and stage root-owned 0600
update-unified.toml with actual host paths. No data/credentials/modules are bundled.

Run preview without installing services or migrating:

```sh
/usr/bin/python3 -I /root/talocart-install/install_server.py --config-source /root/update-unified.toml --root-public-key /root/trusted-updater-root-public.pem --nginx-site /etc/nginx/sites-available/shop
```

After reviewing preview, add `--apply`. Success requires verified base migration,
doctor/startup and updater scheduling. Complete setup, login and EN/IT smoke
before opening access. Subsequent approved releases use owner-requested backend
updates, not this first-install command.

Existing/interrupted installations require administrative recovery. The installer
never resets data or overwrites a partial installation. An independent systemd
guard stops the application if the installer dies and blocks startup after reboot
until durable success. Error cleanup stops the service even when full/read-only
storage prevents writing logs/markers. Preserve journal/data for recovery.

Full source guide: `docs/guides/application-updates.md` in the verified release.
