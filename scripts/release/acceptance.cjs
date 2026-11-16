// Whole-database, synthetic acceptance drill. Requires CREATEDB on a dedicated test DB.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const https = require('node:https');
const { once } = require('node:events');
const { spawn, spawnSync } = require('node:child_process');
const { randomBytes, createHash } = require('node:crypto');
const { Client } = require('pg');
const { chromium } = require('playwright');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

(async () => {
  const testURL = new URL(process.env.TALOCART_TEST_DATABASE_URL);
  assert.match(testURL.pathname, /test/i, 'Dedicated test database required');
  assert.equal(testURL.search, '', 'Whole database drill does not accept search_path options');
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-release-')));
  const output = path.resolve(process.env.TALOCART_RELEASE_OUTPUT || '.local/verification/phase7/acceptance');
  fs.mkdirSync(output, { recursive: true });
  const python = process.env.PYTHON || 'python3';
  const operations = path.resolve(process.env.TALOCART_RELEASE_OPERATIONS || 'scripts/release/operations.py');
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith('TALOCART_') && !k.startsWith('PG')));
  env.TALOCART_LOG = 'warn';
  if (process.env.RELEASE_TEST_PG_CONTAINER) {
    env.RELEASE_TEST_PG_CONTAINER = process.env.RELEASE_TEST_PG_CONTAINER;
    const clients = path.join(root, 'pg-client'); fs.mkdirSync(clients);
    for (const tool of ['psql', 'pg_dump', 'pg_restore']) {
      const adapter = path.resolve('scripts/release/pg-container.py');
      const quote = s => "'" + s.replaceAll("'", "'\\''") + "'";
      fs.writeFileSync(path.join(clients, tool), `#!/bin/sh\nexec ${quote(python)} ${quote(adapter)} ${tool} "$@"\n`, { mode: 0o700 });
    }
    env.PATH = clients + path.delimiter + process.env.PATH;
  }
  function command(executable, args, expected = 0) {
    console.log(`Command: ${path.basename(executable)} ${args[0]}`);
    const result = spawnSync(executable, args, { env, cwd: root, encoding: 'utf8', timeout: 300000 });
    assert.equal(result.status, expected, `${path.basename(executable)} ${args[0]} unexpected result; private diagnostics omitted`);
    return result;
  }
  const owner = new Client({ connectionString: testURL.href });
  const suffix = `${process.pid}_${randomBytes(4).toString('hex')}`;
  const names = ['source', 'restore', 'fresh', 'failed'].map(n => `talocart_rc_${n}_${suffix}`);
  const created = [];
  const urls = names.map(name => { const u = new URL(testURL); u.pathname = '/' + name; return u.href; });
  const localOrigin = 'http://127.0.0.1:3128';
  const publicOrigin = 'https://shop.example.test:3448';
  let server, proxy, browser;
  const report = { schema_contract: 'model-sku', clean_install: false, migration_idempotency: false, restore: false, https_profiles: [] };
  function config(name, url, secure = false) {
    const target = path.join(root, name + '.toml');
    fs.writeFileSync(target, `[server]\nlisten="127.0.0.1:3128"\nsecure_cookies=${secure}\n${secure ? `public_origin="${publicOrigin}"\n` : ''}[database]\nurl=${JSON.stringify(url)}\n[media]\nroot=${JSON.stringify(path.join(root, name + '-media'))}\n[packages]\nroot=${JSON.stringify(path.join(root, name + '-packages'))}\n`, { mode: 0o600 });
    return target;
  }
  const sourceConfig = config('source', urls[0]);
  const restoreConfig = config('restored', urls[1], true);
  const freshConfig = config('fresh', urls[2]);
  async function query(url, sql, values) {
    const client = new Client({ connectionString: url });
    await client.connect();
    try { return await client.query(sql, values); } finally { await client.end(); }
  }
  async function snapshot(url, legacy = false) {
    const client = new Client({ connectionString: url }); await client.connect();
    const result = {};
    try {
      await client.query('BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY');
      const tables = (await client.query("SELECT table_name FROM information_schema.tables WHERE table_schema='public' AND table_type='BASE TABLE' ORDER BY table_name")).rows;
      for (const { table_name: name } of tables) {
        assert.match(name, /^[a-z_]+$/);
        const projection = legacy && name === 'storefront_settings' ? "to_jsonb(t)-'theme_id'" : 'to_jsonb(t)';
        const data = (await client.query(`SELECT count(*)::text AS count, md5(coalesce(string_agg((${projection})::text,chr(10) ORDER BY (${projection})::text),'')) AS digest FROM ${name} t`)).rows[0];
        result[name] = data;
      }
      await client.query('COMMIT');
      return result;
    } finally { await client.end(); }
  }
  function mediaHashes(folder) {
    const result = {};
    const walk = dir => { for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else { assert.ok(entry.isFile()); result[path.relative(folder, full)] = createHash('sha256').update(fs.readFileSync(full)).digest('hex'); }
    } };
    walk(folder); return result;
  }
  async function start(executable, selected) {
    const fd = fs.openSync(path.join(output, 'server.log'), 'a', 0o600);
    server = spawn(executable, ['--config', selected, 'serve'], { env, cwd: root, stdio: ['ignore', fd, fd] }); fs.closeSync(fd);
    for (let i = 0; i < 150; i++) {
      assert.equal(server.exitCode, null, 'Release server exited');
      try { if ((await fetch(localOrigin + '/health/ready')).ok) return; } catch {}
      await sleep(100);
    }
    throw Error('Readiness timeout');
  }
  async function stop() {
    if (server && server.exitCode === null) {
      const ended = once(server, 'exit'); server.kill('SIGTERM');
      let timer;
      try {
        const [code] = await Promise.race([ended, new Promise((_, reject) => { timer = setTimeout(() => reject(Error('Shutdown timeout')), 20000); })]);
        assert.equal(code, 0, 'Graceful shutdown');
      } finally { clearTimeout(timer); }
    }
    server = undefined;
  }
  async function bootstrap(selected, executable) {
    await start(executable, selected);
    const context = await browser.newContext({ javaScriptEnabled: false });
    try {
      await context.addCookies([{ name: 'talocart_admin_language', value: 'it', url: localOrigin }]);
      const page = await context.newPage(); await page.goto(localOrigin + '/setup');
      await page.getByLabel('Nome del negozio', { exact: true }).fill('Release test');
      await page.getByLabel('Email amministratore').fill('owner@example.test');
      await page.getByLabel('Password', { exact: true }).fill('Release-test-password-2026');
      await page.getByRole('button', { name: 'Crea negozio', exact: true }).click();
      await page.waitForURL('**/admin/login');
    } finally { await context.close(); await stop(); }
  }
  try {
    await owner.connect();
    for (const name of names) { await owner.query(`CREATE DATABASE ${name}`); created.push(name); }
    browser = await chromium.launch({ channel: process.env.TALOCART_BROWSER_CHANNEL || undefined,
      args: ['--host-resolver-rules=MAP shop.example.test 127.0.0.1', '--no-proxy-server'] });
    // Fresh release: doctor/serve reject an empty schema, migration is explicit and repeatable.
    command(binary, ['--config', freshConfig, 'doctor'], 1);
    command(binary, ['--config', freshConfig, 'serve'], 1);
    command(binary, ['--config', freshConfig, 'migrate']);
    command(binary, ['--config', freshConfig, 'migrate']);
    command(binary, ['--config', freshConfig, 'doctor']);
    await bootstrap(freshConfig, binary); report.clean_install = true;
    command(binary, ['--config', sourceConfig, 'migrate']);
    await bootstrap(sourceConfig, binary);
    command(binary, ['--config', sourceConfig, 'seed-demo', '--development']);
    await query(urls[0], "INSERT INTO checkout_countries(country_code) VALUES('IT'); UPDATE shipping_methods SET active=true WHERE id=2; INSERT INTO shipping_method_countries(method_id,country_code) VALUES(2,'IT'); UPDATE payment_methods SET active=true WHERE id=2; INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Recovery item','RECOVERY-TEST','recovery-item',1000,10,'published',(SELECT min(id) FROM tax_classes WHERE active)); UPDATE email_configuration SET enabled=false,from_name='Synthetic shop',from_email='sender@example.test',reply_to='support@example.test',admin_recipients=ARRAY['shop@example.test']");
    await start(binary, sourceConfig);
    const buyer = await browser.newContext({ javaScriptEnabled: false });
    try {
      const page = await buyer.newPage();
      await page.goto(localOrigin + '/products/recovery-item');
      await page.getByRole('button', { name: 'Aggiungi al carrello', exact: true }).click();
      await page.goto(localOrigin + '/checkout');
      for (const [key, value] of Object.entries({ email: 'buyer@example.test', first_name: 'Alex', last_name: 'Example', phone: '+393331234567', shipping_street: 'Via Example', shipping_number: '1', shipping_postal: '00100', shipping_city: 'Roma', shipping_region: 'RM' })) await page.locator('#' + key).fill(value);
      await page.locator('#shipping_country').selectOption('IT');
      await page.locator('button[value=continue]').click(); await page.waitForURL('**/checkout/methods');
      await page.locator('[name=shipping_method_id][value="2"]').check();
      await page.locator('[name=payment_method_id][value="2"]').check();
      await page.locator('button[value=update]').click(); await page.waitForLoadState('load');
      const confirmed = page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === '/checkout/methods');
      await page.locator('button[value=confirm]').click();
      const status = (await confirmed).status();
      if (status !== 303) console.log('Checkout validation: ' + (await page.locator('.ui-message,.ui-field-error').allTextContents()).join(' | '));
      assert.equal(status, 303, 'Order confirmation'); await page.waitForURL('**/checkout/orders/ORD-*');
      assert.equal((await query(urls[0], 'SELECT count(*) FROM orders')).rows[0].count, '1');
      assert.equal((await query(urls[0], 'SELECT stock FROM catalog_products WHERE model=\'RECOVERY-TEST\'')).rows[0].stock, 9);
      report.order_snapshot_and_stock = true;
    } finally { await buyer.close(); await stop(); }
    command(binary, ['--config', sourceConfig, 'doctor']);
    const before = await snapshot(urls[0], true);
    const originalMedia = mediaHashes(path.join(root, 'source-media'));
    command(binary, ['--config', sourceConfig, 'migrate']);
    command(binary, ['--config', sourceConfig, 'doctor']);
    const after = await snapshot(urls[0], true);
    for (const [table, data] of Object.entries(before)) if (table !== '_sqlx_migrations') assert.deepEqual(after[table], data, 'Repeated migration preserved ' + table);
    assert.deepEqual(mediaHashes(path.join(root, 'source-media')), originalMedia);
    assert.equal((await query(urls[0], 'SELECT theme_id FROM storefront_settings')).rows[0].theme_id, 'default');
    report.migration_idempotency = true;
    config('source', urls[0], true);
    command('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', path.join(root, 'key.pem'), '-out', path.join(root, 'cert.pem'), '-days', '1', '-subj', '/CN=shop.example.test']);
    proxy = https.createServer({ key: fs.readFileSync(path.join(root, 'key.pem')), cert: fs.readFileSync(path.join(root, 'cert.pem')) }, (req, res) => {
      const upstream = http.request(localOrigin + req.url, { method: req.method, headers: { ...req.headers, host: 'shop.example.test:3448' } }, incoming => { res.writeHead(incoming.statusCode, incoming.headers); incoming.pipe(res); });
      upstream.on('error', () => { res.writeHead(502); res.end(); }); req.pipe(upstream);
    });
    proxy.listen(3448, '127.0.0.1'); await once(proxy, 'listening');
    async function profiles(selected) {
      await start(binary, selected);
      try {
        for (const locale of ['it', 'en']) for (const js of [true, false]) {
          await query(selected === sourceConfig ? urls[0] : urls[1], 'UPDATE administrators SET locale=$1', [locale]);
          const context = await browser.newContext({ ignoreHTTPSErrors: true, javaScriptEnabled: js, viewport: { width: js ? 390 : 1440, height: 1000 } });
          try {
            const page = await context.newPage(); const errors = []; page.on('pageerror', e => errors.push(e.message));
            const prefix = locale === 'it' ? '' : '/en';
            await page.goto(publicOrigin + prefix + '/catalog');
            assert.equal(await page.locator('html').getAttribute('lang'), locale);
            assert.ok((await page.locator('link[rel=canonical]').getAttribute('href')).startsWith(publicOrigin));
            assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
            await context.addCookies([{ name: 'talocart_admin_language', value: locale, url: publicOrigin, secure: true }]);
            await page.goto(publicOrigin + '/admin/login');
            await page.getByLabel(locale === 'it' ? 'Email amministratore' : 'Administrator email').fill('owner@example.test');
            await page.getByLabel('Password', { exact: true }).fill('Release-test-password-2026');
            await page.getByRole('button', { name: locale === 'it' ? 'Accedi' : 'Sign in', exact: true }).click();
            await page.waitForURL('**/admin');
            const cookies = await context.cookies();
            assert.ok(cookies.some(c => c.name === 'talocart_session' && c.httpOnly && c.secure && c.sameSite === 'Strict'), 'Secure session over HTTPS');
            await page.goto(publicOrigin + '/admin/modules');
            assert.equal(await page.locator('html').getAttribute('lang'), locale);
            await page.screenshot({ path: path.join(output, `${path.basename(selected)}-${locale}-${js ? 'mobile' : 'desktop-no-js'}.png`), fullPage: true });
            assert.deepEqual(errors, []);
            report.https_profiles.push({ target: path.basename(selected), locale, js, secure_session: true });
          } finally { await context.close(); }
        }
      } finally { await stop(); }
    }
    await profiles(sourceConfig);
    const sourceSnapshot = await snapshot(urls[0]);
    const sourceFiles = mediaHashes(path.join(root, 'source-media'));
    const savedConfig = fs.readFileSync(sourceConfig);
    const backup = path.join(root, 'backup');
    command(python, [operations, 'backup', '--config', sourceConfig, '--binary', binary, '--output', backup]);
    command(python, [operations, 'verify', backup]);
    assert.deepEqual(await snapshot(urls[0]), sourceSnapshot, 'Backup never mutates source DB');
    assert.deepEqual(fs.readFileSync(sourceConfig), savedConfig);
    assert.deepEqual(mediaHashes(path.join(root, 'source-media')), sourceFiles);
    assert.equal(fs.statSync(path.join(backup, 'talocart.toml')).mode & 0o777, 0o600);
    // A real pg_restore failure must preserve complete media and leave the destination database empty.
    const failedConfig = config('failed', urls[3]);
    const invalidBackup = path.join(root, 'invalid-backup');
    fs.cpSync(backup, invalidBackup, { recursive: true });
    fs.writeFileSync(path.join(invalidBackup, 'database.dump'), 'synthetic invalid PostgreSQL archive');
    const manifestPath = path.join(invalidBackup, 'backup.json');
    const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
    manifest.files['database.dump'] = createHash('sha256').update(fs.readFileSync(path.join(invalidBackup, 'database.dump'))).digest('hex');
    fs.writeFileSync(manifestPath, JSON.stringify(manifest));
    const failure = command(python, [operations, 'restore', '--backup', invalidBackup, '--config', failedConfig], 1);
    assert.match(failure.stderr, /Media preserved/);
    assert.deepEqual(mediaHashes(path.join(root, 'failed-media')), sourceFiles);
    assert.deepEqual(await snapshot(urls[3]), {}, 'Failed pg_restore leaves empty database');
    command(python, [operations, 'restore', '--backup', invalidBackup, '--config', failedConfig], 1);
    assert.deepEqual(mediaHashes(path.join(root, 'failed-media')), sourceFiles);
    report.failed_restore_preserves_media = true;
    command(python, [operations, 'restore', '--backup', backup, '--config', restoreConfig]);
    assert.deepEqual(await snapshot(urls[1]), sourceSnapshot, 'Every restored table matches');
    assert.deepEqual(mediaHashes(path.join(root, 'restored-media')), sourceFiles);
    command(binary, ['--config', restoreConfig, 'doctor']);
    command(python, [operations, 'restore', '--backup', backup, '--config', restoreConfig], 1);
    const occupiedConfig = config('occupied', urls[1], true);
    command(python, [operations, 'restore', '--backup', backup, '--config', occupiedConfig], 1);
    assert.ok(!fs.existsSync(path.join(root, 'occupied-media')));
    assert.deepEqual(await snapshot(urls[1]), sourceSnapshot, 'Refused overwrite leaves data intact');
    await profiles(restoreConfig);
    // A running writer prevents a coordinated backup; an extra file prevents verification.
    await start(binary, sourceConfig);
    command(python, [operations, 'backup', '--config', sourceConfig, '--binary', binary, '--output', path.join(root, 'busy-backup')], 1);
    assert.ok(!fs.existsSync(path.join(root, 'busy-backup'))); await stop();
    fs.writeFileSync(path.join(backup, 'injected'), 'unexpected');
    command(python, [operations, 'verify', backup], 1);
    report.restore = true;
    report.restored_tables = Object.keys(sourceSnapshot).length;
    report.media_files = Object.keys(sourceFiles).length;
    fs.writeFileSync(path.join(output, 'result.json'), JSON.stringify(report, null, 2) + '\n');
    console.log(JSON.stringify(report));
  } finally {
    await stop();
    if (proxy) { proxy.closeAllConnections(); await new Promise(resolve => proxy.close(resolve)); }
    if (browser) await browser.close();
    for (const name of created) await owner.query(`DROP DATABASE ${name} WITH (FORCE)`);
    await owner.end();
    fs.rmSync(root, { recursive: true, force: true });
  }
})().catch(error => { console.error('Release acceptance failed: ' + error.name + ' code=' + (error.code || '') + ' constraint=' + (error.constraint || '')); console.error(String(error.stack).split('\n').filter(line => /^\s+at .*acceptance/.test(line)).join('\n')); process.exitCode = 1; });
