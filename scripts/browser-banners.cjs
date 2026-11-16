// Native banner CRUD and responsive slideshow against a disposable test schema/media directory.
const assert = require('node:assert/strict'), fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { randomBytes } = require('node:crypto'), { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events'), { Client } = require('pg'), { chromium } = require('playwright');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test DB required');
  const db = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL }); await db.connect();
  let browser;
  try { browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {}); }
  catch (error) { await db.end(); throw error; }
  const schema = 'talocart_banners_' + process.pid + '_' + Date.now(), folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-banners-'));
  const origin = 'http://127.0.0.1:3128', output = path.resolve('.local/verification/banners/browser');
  fs.mkdirSync(output, { recursive: true });
  const url = new URL(process.env.TALOCART_TEST_DATABASE_URL); url.searchParams.set('options', '-csearch_path=' + schema);
  const config = path.join(folder, 'talocart.toml'); fs.writeFileSync(config, '[server]\nlisten="127.0.0.1:3128"\n', { mode: 0o600 });
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
  const env = { ...process.env, TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: '127.0.0.1:3128', TALOCART_MEDIA_ROOT: path.join(folder, 'media'), TALOCART_LOG: 'warn' };
  let server; const results = [];
  const start = async () => {
    const fd = fs.openSync(path.join(output, 'server.log'), 'a', 0o600);
    server = spawn(binary, ['--config', config, 'serve'], { env, stdio: ['ignore', fd, fd] }); fs.closeSync(fd);
    for (let i = 0; i < 150; i++) { try { if ((await fetch(origin + '/health/ready')).ok) return; } catch {} await new Promise(r => setTimeout(r, 100)); }
    throw Error('Banner fixture readiness failed');
  };
  const stop = async () => { if (server && server.exitCode === null) { const done = once(server, 'exit'); server.kill('SIGTERM'); await done; } };
  try {
    await db.query('CREATE SCHEMA ' + schema); await db.query('SET search_path TO ' + schema);
    assert.equal(spawnSync(binary, ['--config', config, 'migrate'], { env, stdio: 'pipe' }).status, 0, 'Isolated migration');
    await start();
    const setup = await browser.newContext(); const page = await setup.newPage();
    const password = randomBytes(20).toString('hex');
    await setup.addCookies([{ name: 'talocart_admin_language', value: 'it', url: origin }]);
    await page.goto(origin + '/setup');
    await page.getByLabel('Nome del negozio', { exact: true }).fill('Banner test');
    await page.getByLabel('Email amministratore').fill('owner@example.test'); await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByRole('button', { name: 'Crea negozio', exact: true }).click(); await page.waitForURL('**/admin/login');
    await page.getByLabel('Email amministratore').fill('owner@example.test'); await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByRole('button', { name: 'Accedi', exact: true }).click(); await page.waitForURL('**/admin');
    const cookies = (await setup.cookies()).filter(c => c.name === 'talocart_session');
    await page.goto(origin + '/admin/banners/new'); const csrf = await page.locator('#banner-editor [name=csrf]').inputValue();
    const languages = await setup.request.post(origin + '/admin/settings/storefront/languages', { data: new URLSearchParams({ csrf, primary_locale: 'en' }).toString() + '&locale=en&locale=it', headers: { 'content-type': 'application/x-www-form-urlencoded' }, maxRedirects: 0 });
    assert.equal(languages.status(), 303, 'Explicit bilingual fixture');
    const art = await browser.newPage({ viewport: { width: 2400, height: 800 } });
    await art.setContent('<style>body{margin:0}</style><svg xmlns="http://www.w3.org/2000/svg" width="2400" height="800"><rect width="2400" height="800" fill="#ececda"/><circle cx="1800" cy="400" r="280" fill="#476647"/><text x="160" y="370" font-size="120" fill="#202d25">Catalog / Catalogo</text><text x="160" y="490" font-size="50" fill="#476647">Responsive banner fixture</text></svg>');
    const artwork = await art.locator('svg').screenshot(); await art.close();
    const upload = await setup.request.post(origin + '/admin/media/upload', { multipart: { csrf, images: { name: 'banner-artwork.png', mimeType: 'image/png', buffer: artwork } }, maxRedirects: 0 });
    assert.equal(upload.status(), 303);
    const image = (await db.query("SELECT m.id FROM media_objects m JOIN media_library l ON l.key=m.key WHERE l.name='banner-artwork.png'")).rows[0].id;
    for (let i = 0; i < 40; i++) {
      const r = await setup.request.post(origin + '/admin/media/upload', { multipart: { csrf, images: { name: 'picker-' + String(i).padStart(2, '0') + '.png', mimeType: 'image/png', buffer: fs.readFileSync('crates/web/tests/fixtures/avatar.png') } }, maxRedirects: 0 }); assert.equal(r.status(), 303);
    }
    await setup.close();
    for (const [profile, width, js] of [['desktop', 1440, true], ['mobile', 390, true], ['no-js', 1440, false], ['mobile-no-js', 390, false]]) {
      for (const locale of ['en', 'it']) {
        await db.query('UPDATE administrators SET locale=$1', [locale]); await db.query('UPDATE home_banners SET active=false');
        const context = await browser.newContext({ viewport: { width, height: 1000 }, deviceScaleFactor: width < 700 ? 2 : 1, javaScriptEnabled: js });
        await context.addCookies(cookies); const page = await context.newPage(), errors = []; page.on('pageerror', e => errors.push(e.message));
        try {
          const submit = async (action, status) => {
            const done = page.waitForResponse(r => r.request().method() === 'POST' && (new URL(r.url()).pathname.startsWith('/admin/banners/') || new URL(r.url()).pathname === '/admin/media'));
            await page.locator('button[name=action][value="' + action + '"]:visible').click();
            assert.equal((await done).status(), status, action); await page.waitForLoadState('load');
          };
          await page.goto(origin + '/admin/banners/new');
          assert.equal(await page.locator('html').getAttribute('lang'), locale);
          await page.locator('#banner-label').fill('Test ' + profile + ' ' + locale);
          await page.locator('#banner-name').fill('Summer offers');
          await page.locator('#banner-content-it-name').fill('Offerte estate');
          await page.locator('#banner-description').fill('/catalog');
          await page.locator('#banner-content-it-description').fill('/brands');
          await page.locator('#banner-active').check();
          const implicit = page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === '/admin/banners/new');
          await page.locator('#banner-name').press('Enter');
          assert.equal((await implicit).status(), 422, 'Enter submits Save, not Browse'); await page.waitForLoadState('load');
          assert.equal(await page.locator('#banner-image-error').count(), 1);
          await submit('browse', 200); assert.equal(await page.locator('.banner-media-choice').count(), 40);
          await submit('media_next', 200); assert.equal(await page.locator('.banner-media-choice').count(), 1);
          await page.locator('#banner-media-query').fill('banner-artwork'); const search = page.waitForResponse(r => r.request().method() === 'POST' && (new URL(r.url()).pathname.startsWith('/admin/banners/') || new URL(r.url()).pathname === '/admin/media')); await page.locator('#banner-media-query').press('Enter'); assert.equal((await search).status(), 200); await page.waitForLoadState('load'); assert.equal(await page.locator('.banner-media-choice').count(), 1);
          await submit('choose_' + image, 200);
          assert.equal(await page.locator('#banner-label').inputValue(), 'Test ' + profile + ' ' + locale);
          assert.equal(await page.locator('#banner-content-it-name').inputValue(), 'Offerte estate');
          await page.locator('#banner-description').fill('javascript:alert(1)'); await submit('save', 422);
          assert.equal(await page.locator('#banner-description').getAttribute('aria-invalid'), 'true');
          await page.locator('#banner-description').fill('/catalog'); await submit('save', 303);
          const editor = page.url(), id = Number(new URL(editor).pathname.split('/').pop());
          // API request creates the second banner through the same ordinary native form handler.
          const second = await context.request.post(origin + '/admin/banners/new', { form: { csrf, revision: '0', label: 'Second ' + profile + locale, name: 'Second banner', description: '', image_id: String(image), sort_order: '1', active: 'true', content_primary_locale: 'en', content_active_locales: 'en,it' }, maxRedirects: 0 }); assert.equal(second.status(), 303);
          await page.screenshot({ path: path.join(output, profile + '-' + locale + '-admin.png'), fullPage: true });
          const home = origin + (locale === 'it' ? '/it' : '/');
          for (const theme of ['default', 'essential']) {
           for (const density of ['comfortable', 'compact']) {
            await db.query('UPDATE storefront_settings SET theme_id=$1,density=$2', [theme,density]);
            await page.goto(home);
            assert.equal(await page.locator('.hero').count(), 0); assert.equal(await page.locator('h1').count(), 1);
            assert.equal(await page.locator('[data-banner]').count(), 2);
            assert.equal(await page.locator('[data-banner]:visible img').getAttribute('alt'), locale === 'it' ? 'Offerte estate' : 'Summer offers');
            assert.equal(await page.locator('[data-banner]:visible a').getAttribute('href'), locale === 'it' ? '/it/brands' : '/catalog');
            const imageElement = page.locator('[data-banner]:visible img');
            await imageElement.evaluate(el => el.decode());
            assert.equal(await imageElement.evaluate(el => el.naturalWidth > 0), true);
            assert.equal(await imageElement.evaluate(el => getComputedStyle(el).objectFit), 'contain');
            assert.ok((await imageElement.getAttribute('srcset')).includes('512w'));
            const layout = await imageElement.evaluate(el => {
              const size = el.sizes, probe = document.createElement('div');
              probe.style.width = size; probe.style.position = 'absolute'; document.body.append(probe);
              const expected = probe.getBoundingClientRect().width; probe.remove();
              return { expected, actual: el.getBoundingClientRect().width };
            }); assert.ok(Math.abs(layout.expected - layout.actual) < 2, JSON.stringify(layout));
            assert.equal(await page.locator('[data-banner-controls]').isVisible(), js);
            if (js) {
              await page.locator('[data-banner-next]').click(); assert.equal(await page.locator('[data-banner]:visible a').count(), 0);
              await page.locator('[data-banner-prev]').focus(); await page.keyboard.press('ArrowRight');
              assert.equal(await page.locator('[data-banner-index="0"]').getAttribute('aria-current'), 'true');
              assert.equal(await page.locator('[data-banner-toggle]').innerText(), locale === 'it' ? 'Riprendi' : 'Play');
              // A touch swipe changes slide and suppresses the accidental link click.
              await page.locator('.home-banners-stage').dispatchEvent('pointerdown', { pointerId: 1, pointerType: 'touch', isPrimary: true, clientX: 250, clientY: 90 });
              await page.locator('.home-banners-stage').dispatchEvent('pointerup', { pointerId: 1, pointerType: 'touch', isPrimary: true, clientX: 130, clientY: 95 });
              assert.equal(await page.locator('[data-banner-index="1"]').getAttribute('aria-current'), 'true');
              await page.keyboard.press('Home');
            }
            assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
            await page.screenshot({ path: path.join(output, profile + '-' + locale + '-' + theme + '-' + density + '.png'), fullPage: true });
           }
          }
          if (js) {
            await page.emulateMedia({ reducedMotion: 'reduce' }); await page.goto(home);
            assert.equal(await page.locator('[data-banner-toggle]').innerText(), locale === 'it' ? 'Riprendi' : 'Play');
            await page.emulateMedia({ reducedMotion: 'no-preference' }); await page.clock.install(); await page.goto(home); await page.mouse.move(0, 0);
            await page.clock.fastForward(6100); assert.equal(await page.locator('[data-banner-index="1"]').getAttribute('aria-current'), 'true', 'Autoplay advances');
            await page.locator('[data-banner-toggle]').click(); assert.equal(await page.locator('[data-banner-toggle]').innerText(), locale === 'it' ? 'Riprendi' : 'Play', 'Pause works on first click');
            await page.clock.fastForward(12000); assert.equal(await page.locator('[data-banner-index="1"]').getAttribute('aria-current'), 'true');
            await page.locator('[data-banner-toggle]').click(); await page.mouse.move(0, 0); await page.clock.fastForward(6100);
            assert.equal(await page.locator('[data-banner-index="0"]').getAttribute('aria-current'), 'true');
            await page.locator('[data-banner]:visible a').focus(); await page.clock.fastForward(12000); assert.equal(await page.locator('[data-banner-index="0"]').getAttribute('aria-current'), 'true', 'Focus pauses persistently');
          }
          await page.goto(editor); await page.locator('#banner-active').uncheck(); await submit('save', 303);
          await page.goto(home); assert.equal(await page.locator('[data-banner-controls]').count(), 0);
          await page.goto(editor); await page.locator('.information-delete summary').click(); await submit('delete', 303);
          assert.equal((await db.query('SELECT count(*)::int AS n FROM home_banners WHERE id=$1', [id])).rows[0].n, 0);
          assert.equal((await db.query('SELECT count(*)::int AS n FROM media_library l JOIN media_objects m ON m.key=l.key WHERE m.id=$1', [image])).rows[0].n, 1);
          assert.deepEqual(errors, []); results.push(profile + ' ' + locale + ': CRUD, draft picker/search/pagination, validation, both themes, responsive/cache, controls and no-JS OK');
        } finally { await context.close(); }
      }
    }
    await stop(); await start();
    const r = await fetch(origin + '/'); assert.equal(r.status, 200); assert.ok((await r.text()).includes('data-banners'), 'Banners persist after restart');
    fs.writeFileSync(path.join(output, 'results.txt'), results.join('\n') + '\n'); console.log(results.join('\n'));
  } finally {
    await stop(); await browser.close(); await db.query('SET search_path TO public'); await db.query('DROP SCHEMA IF EXISTS ' + schema + ' CASCADE');
    await db.end(); fs.rmSync(folder, { recursive: true, force: true });
  }
})().catch(e => { console.error(e); process.exitCode = 1; });
