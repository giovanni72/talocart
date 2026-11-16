// Publication regressions: real browser cache and HTTP while settings are locked.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const {randomBytes} = require('node:crypto');
const {spawn, spawnSync} = require('node:child_process');
const {once} = require('node:events');
const {Client} = require('pg');
const {chromium} = require('playwright');

(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test DB required');
  const db = new Client({connectionString: process.env.TALOCART_TEST_DATABASE_URL});
  await db.connect();
  const schema = `talocart_indexing_${process.pid}_${Date.now()}`;
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-indexing-'));
  const failures = [];
  let server, browser;
  const check = async (name, run) => {
    try { await run(); console.log(`PASSED ${name}`); }
    catch (error) { failures.push(name); console.error(`FAILED ${name}: ${error.message}`); }
  };
  try {
    await db.query(`CREATE SCHEMA ${schema}`);
    await db.query(`SET search_path TO ${schema}`);
    const lease = net.createServer();
    await new Promise(resolve => lease.listen(0, '127.0.0.1', resolve));
    const port = lease.address().port;
    await new Promise(resolve => lease.close(resolve));
    const origin = `http://127.0.0.1:${port}`;
    const config = path.join(folder, 'talocart.toml');
    fs.writeFileSync(config, `[server]\nlisten="127.0.0.1:${port}"\n`, {mode: 0o600});
    const url = new URL(process.env.TALOCART_TEST_DATABASE_URL);
    url.searchParams.set('options', `-csearch_path=${schema}`);
    const env = {...process.env, TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: `127.0.0.1:${port}`,
      TALOCART_MEDIA_ROOT: path.join(folder, 'media'), TALOCART_LOG: 'warn'};
    const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/debug/talocart');
    const migrated = spawnSync(binary, ['--config', config, 'migrate'], {env, encoding: 'utf8'});
    assert.equal(migrated.status, 0, 'Isolated migration');
    const log = fs.openSync(path.join(folder, 'server.log'), 'a', 0o600);
    server = spawn(binary, ['--config', config, 'serve'], {env, stdio: ['ignore', log, log]});
    fs.closeSync(log);
    let ready = false;
    for (let i = 0; i < 100; i++) {
      try { if ((await fetch(origin + '/health/ready')).ok) { ready = true; break; } } catch {}
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.ok(ready, 'Synthetic server readiness');

    await check('embedded assets stay available without settings reads', async () => {
      const initial = await fetch(origin + '/assets/ui.css');
      const etag = initial.headers.get('etag');
      await initial.arrayBuffer();
      await db.query('BEGIN');
      try {
        await db.query('LOCK TABLE storefront_settings IN ACCESS EXCLUSIVE MODE');
        for (const route of ['/assets/ui.css', '/assets/settings.js', '/favicon.svg']) {
          for (const method of ['GET', 'HEAD']) {
            const response = await fetch(origin + route, {method, signal: AbortSignal.timeout(2000)});
            assert.equal(response.status, 200, `${method} ${route}`);
            assert.equal(response.headers.get('x-robots-tag'), 'noindex, nofollow');
            await response.arrayBuffer();
          }
        }
        const cached = await fetch(origin + '/assets/ui.css', {
          headers: {'If-None-Match': etag}, signal: AbortSignal.timeout(2000),
        });
        assert.equal(cached.status, 304);
        assert.equal(cached.headers.get('x-robots-tag'), 'noindex, nofollow');
      } finally { await db.query('ROLLBACK'); }
    });

    browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? {channel: process.env.TALOCART_BROWSER_CHANNEL} : {});
    const context = await browser.newContext();
    await context.addCookies([{name: 'talocart_admin_language', value: 'it', url: origin}]);
    const page = await context.newPage();
    // Seed a real public image through authorized forms; never intercept routes,
    // since Playwright routing disables the HTTP cache this regression needs.
    const password = randomBytes(20).toString('hex');
    await page.goto(origin + '/setup');
    await page.getByLabel('Nome del negozio', {exact: true}).fill('Indexing test');
    await page.getByLabel('Email amministratore').fill('owner@example.test');
    await page.getByLabel('Password', {exact: true}).fill(password);
    await page.getByRole('button', {name: 'Crea negozio', exact: true}).click();
    await page.waitForURL('**/admin/login');
    await page.getByLabel('Email amministratore').fill('owner@example.test');
    await page.getByLabel('Password', {exact: true}).fill(password);
    await page.getByRole('button', {name: 'Accedi', exact: true}).click();
    await page.waitForURL('**/admin');
    await page.goto(origin + '/admin/settings');
    const csrf = await page.locator('#settings-editor [name=csrf]').inputValue();
    const uploaded = await context.request.post(origin + '/admin/settings/branding/logo', {
      multipart: {csrf, image: {name: 'indexing.png', mimeType: 'image/png', buffer: fs.readFileSync('crates/web/tests/fixtures/avatar.png')}}, maxRedirects: 0,
    });
    assert.equal(uploaded.status(), 303);
    const image = (await db.query('SELECT m.id,m.key FROM media_objects m JOIN store_settings s ON s.logo_key=m.key WHERE s.id=1')).rows[0];
    const target = `/media/public/${image.id}/${encodeURIComponent(image.key)}/original`;
    await page.goto(origin + '/health/live');
    await check('304 responses replace cached indexing directives in both directions', async () => {
      const wire = [];
      const cdp = await context.newCDPSession(page);
      await cdp.send('Network.enable');
      cdp.on('Network.responseReceivedExtraInfo', event => {
        const etag = Object.entries(event.headers).find(([key]) => key.toLowerCase() === 'etag')?.[1];
        if (etag === `"media-${image.id}"`) wire.push(event.statusCode);
      });
      let etag;
      for (const enabled of [false, true, false, true]) {
        await db.query('UPDATE storefront_settings SET indexing_enabled=$1 WHERE id=1', [enabled]);
        const response = await page.evaluate(async target => {
          const result = await fetch(target);
          await result.arrayBuffer();
          return {status: result.status, robots: result.headers.get('x-robots-tag'), etag: result.headers.get('etag')};
        }, target);
        assert.equal(response.status, 200, 'Browser reconstructs the cached response');
        assert.equal(response.robots, enabled ? 'index, follow' : 'noindex, nofollow', `Cached policy: enabled=${enabled}`);
        etag ??= response.etag;
        assert.equal(response.etag, etag, 'Image bytes and validator stay unchanged');
      }
      assert.deepEqual(wire, [200, 304, 304, 304], 'Transitions use real HTTP cache revalidation');
      await cdp.detach();
    });
    assert.deepEqual(failures, [], 'Publication regressions');
  } finally {
    if (browser) await browser.close();
    if (server && server.exitCode === null) { const done = once(server, 'exit'); server.kill('SIGTERM'); await done; }
    await db.query('SET search_path TO public');
    await db.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);
    await db.end();
    fs.rmSync(folder, {recursive: true, force: true});
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
