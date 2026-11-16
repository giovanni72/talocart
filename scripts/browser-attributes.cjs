// Obligatory base catalog: native multilingual forms and technical specifications.
const assert = require('node:assert/strict'), fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { randomBytes } = require('node:crypto'), { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events'), { Client } = require('pg'), { chromium } = require('playwright');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test database required');
  const db = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL }); await db.connect();
  let browser;
  try { browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {}); } catch (error) { await db.end(); throw error; }
  const schema = 'talocart_attributes_' + process.pid + '_' + Date.now(), folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-attributes-'));
  const origin = 'http://127.0.0.1:3132', output = path.resolve('.local/verification/attributes/browser'); fs.mkdirSync(output, { recursive: true });
  const url = new URL(process.env.TALOCART_TEST_DATABASE_URL); url.searchParams.set('options', '-csearch_path=' + schema);
  const config = path.join(folder, 'talocart.toml'); fs.writeFileSync(config, '[server]\nlisten="127.0.0.1:3132"\n', { mode: 0o600 });
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
  const env = { ...process.env, TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: '127.0.0.1:3132', TALOCART_MEDIA_ROOT: path.join(folder, 'media'), TALOCART_LOG: 'warn' };
  let server; const results = [];
  const start = async () => {
    const fd = fs.openSync(path.join(output, 'server.log'), 'a', 0o600);
    server = spawn(binary, ['--config', config, 'serve'], { env, stdio: ['ignore', fd, fd] }); fs.closeSync(fd);
    for (let i = 0; i < 150; i++) { try { if ((await fetch(origin + '/health/ready')).ok) return; } catch {} await new Promise(r => setTimeout(r, 100)); }
    throw Error('Attributes fixture readiness failed');
  };
  const stop = async () => { if (server && server.exitCode === null) { const done = once(server, 'exit'); server.kill('SIGTERM'); await done; } };
  try {
    await db.query('CREATE SCHEMA ' + schema); await db.query('SET search_path TO ' + schema);
    assert.equal(spawnSync(binary, ['--config', config, 'migrate'], { env, stdio: 'pipe' }).status, 0, 'Clean isolated migration'); await start();
    const setup = await browser.newContext(); const page = await setup.newPage(), password = randomBytes(20).toString('hex');
    await setup.addCookies([{ name: 'talocart_admin_language', value: 'it', url: origin }]); await page.goto(origin + '/setup');
    await page.getByLabel('Nome del negozio', { exact: true }).fill('Specifications'); await page.getByLabel('Email amministratore').fill('owner@example.test'); await page.getByLabel('Password', { exact: true }).fill(password); await page.locator('#install-demo').check();
    await page.getByRole('button', { name: 'Crea negozio', exact: true }).click(); await page.waitForURL('**/admin/login');
    await page.getByLabel('Email amministratore').fill('owner@example.test'); await page.getByLabel('Password', { exact: true }).fill(password); await page.getByRole('button', { name: 'Accedi', exact: true }).click(); await page.waitForURL('**/admin');
    const cookies = (await setup.cookies()).filter(c => c.name === 'talocart_session');
    await page.goto(origin + '/admin/catalog/attributes/new'); assert.equal(await page.locator('h1').innerText(), 'Attributi');
    const csrf = await page.locator('form [name=csrf]').first().inputValue();
    const languages = await setup.request.post(origin + '/admin/settings/storefront/languages', { data: new URLSearchParams({ csrf, primary_locale: 'en' }).toString() + '&locale=en&locale=it', headers: { 'content-type': 'application/x-www-form-urlencoded' }, maxRedirects: 0 }); assert.equal(languages.status(), 303);
    const product = (await db.query("SELECT id,slug FROM catalog_products WHERE model='MUG'")).rows[0]; assert.ok(product);
    await setup.close();
    for (const [profile, width, js] of [['desktop', 1440, true], ['mobile', 390, true], ['no-js', 1440, false], ['mobile-no-js', 390, false]]) {
      for (const locale of ['en', 'it']) {
        await db.query('UPDATE administrators SET locale=$1', [locale]);
        const context = await browser.newContext({ viewport: { width, height: 1000 }, javaScriptEnabled: js }); await context.addCookies(cookies);
        const page = await context.newPage(), errors = []; page.on('pageerror', e => errors.push(e.message));
        try {
          const submit = async (expected = 303, deletion = false) => {
            const done = page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname.includes('/catalog/'));
            await page.locator(deletion ? '.catalog-delete button[type=submit]' : 'main > form button[type=submit]').click();
            assert.equal((await done).status(), expected); await page.waitForLoadState('load');
          };
          const fill = async (name, italian, order = '0') => { await page.locator('#attribute-name').fill(name); await page.locator('#attribute-content-it-name').fill(italian); await page.locator('#attribute-order').fill(order); };
          const suffix = profile + '-' + locale;
          await page.goto(origin + '/admin/catalog/attribute-groups/new'); await fill('Technical ' + suffix, 'Tecniche ' + suffix); await submit();
          const groupEditor = page.url(), group = new URL(groupEditor).pathname.split('/').pop();
          await page.goto(origin + '/admin/catalog/attributes/new'); await page.locator('#attribute-choice').selectOption(group); await fill('Material ' + suffix, 'Materiale ' + suffix); await submit();
          const attributeEditor = page.url(), attribute = new URL(attributeEditor).pathname.split('/').pop();
          await page.goto(origin + `/admin/catalog/products/${product.id}?tab=attributes#panel-attributes`);
          if (js) { assert.equal(await page.locator('#tab-attributes').getAttribute('aria-selected'), 'true'); await page.locator('#tab-attributes').focus(); await page.keyboard.press('ArrowRight'); assert.equal(await page.locator('#tab-image').evaluate(e => e === document.activeElement), true); await page.locator('#tab-attributes').click(); }
          await page.locator('#panel-attributes a[href$="/attributes/new"]').click(); await page.locator('#attribute-choice').selectOption(attribute); await fill('Cotton <script>', 'Cotone <script>', '4');
          // Invalid order follows the real native submit, retaining both language values.
          await page.locator('main > form').evaluate(form => { form.noValidate = true; }); await page.locator('#attribute-order').fill('-1'); await submit(422);
          assert.equal(await page.locator('#attribute-order').getAttribute('aria-invalid'), 'true'); assert.equal(await page.locator('#attribute-name').inputValue(), 'Cotton <script>'); assert.equal(await page.locator('#attribute-content-it-name').inputValue(), 'Cotone <script>');
          await page.locator('#attribute-order').fill('4'); await submit(); const valueEditor = page.url(), assignment = new URL(valueEditor).pathname.split('/').pop();
          await page.locator('#attribute-name').focus(); await page.keyboard.press('Tab'); assert.equal(await page.locator('#attribute-content-it-name').evaluate(e => e === document.activeElement), true); assert.ok(await page.locator('#attribute-content-it-name').evaluate(e => getComputedStyle(e).outlineStyle !== 'none'));
          assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
          await page.screenshot({ path: path.join(output, suffix + '-editor.png'), fullPage: true });
          await page.goto(origin + `/admin/catalog/products/${product.id}/attributes/new`);
          assert.equal(await page.locator(`#attribute-choice option[value="${attribute}"]`).count(), 0, 'Assigned attributes are excluded from creation');
          assert.ok((await page.locator('#attributes-prerequisite').innerText()).includes(locale === 'it' ? 'Non ci sono attributi disponibili' : 'No unassigned attributes'));
          await page.goto(valueEditor);
          // Competing write increments the DB revision; pending text is preserved on conflict.
          await db.query('UPDATE catalog_product_attributes SET sort_order=5 WHERE id=$1', [assignment]); await page.locator('#attribute-name').fill('Pending'); await submit(409); assert.equal(await page.locator('#attribute-name').inputValue(), 'Pending'); assert.equal(await page.locator('#attribute-choice').inputValue(), attribute, 'Assigned attribute survives a conflict');
          await page.goto(valueEditor); await fill('Cotton <script>', 'Cotone <script>', '-1');
          await page.locator('main > form').evaluate(form => { form.noValidate = true; }); await submit(422);
          assert.equal(await page.locator('#attribute-choice').inputValue(), attribute, 'Assigned attribute survives validation errors');
          assert.equal(await page.locator('#attribute-choice option').count(), 2, 'Existing assignment only loads its own attribute');
          await page.locator('#attribute-order').fill('4'); await submit();
          for (const theme of ['default', 'essential']) {
            await db.query('UPDATE storefront_settings SET theme_id=$1', [theme]);
            const publicPath = (locale === 'it' ? '/it' : '') + '/products/' + product.slug; await page.goto(origin + publicPath);
            assert.equal(await page.locator('#product-specifications-heading').innerText(), locale === 'it' ? 'Caratteristiche' : 'Specifications');
            const specifications = page.locator('.product-specifications'); assert.ok(await specifications.evaluate(el => { const form = document.querySelector('.store-cart-add'); return Boolean(form && (form.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING)); })); assert.ok((await specifications.innerText()).includes((locale === 'it' ? 'Materiale ' : 'Material ') + suffix)); assert.ok((await specifications.innerText()).includes(locale === 'it' ? 'Cotone <script>' : 'Cotton <script>')); assert.equal(await specifications.locator('script').count(), 0);
            assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
            await page.screenshot({ path: path.join(output, suffix + '-' + theme + '-public.png'), fullPage: true });
            const partial = await context.request.get(origin + publicPath, { headers: { 'hx-request': 'true' } }); assert.equal(partial.status(), 200); assert.ok((await partial.text()).includes('product-specifications-heading'));
          }
          if (profile === 'desktop' && locale === 'en') {
            await stop(); await start(); await page.goto(valueEditor);
            assert.equal(await page.locator('#attribute-name').inputValue(), 'Cotton <script>');
            assert.equal(await page.locator('#attribute-content-it-name').inputValue(), 'Cotone <script>');
            assert.equal(await page.locator('#attribute-order').inputValue(), '4');
          }
          await page.goto(origin + '/admin/catalog/attributes'); assert.equal(await page.locator('.sidebar-submenu [aria-current=page]').count(), 1); assert.ok((await page.locator('main').innerText()).includes('Material ' + suffix));
          await page.screenshot({ path: path.join(output, suffix + '-registry.png'), fullPage: true });
          await page.goto(attributeEditor); await page.locator('.catalog-delete input[name=confirm]').check(); await submit(409, true); assert.ok(await page.locator('#attributes-error').isVisible());
          await page.goto(valueEditor); await page.locator('.catalog-delete input[name=confirm]').check(); await submit(303, true); assert.ok(page.url().includes('tab=attributes')); assert.equal(await page.locator('#panel-attributes').getByText('Material ' + suffix, { exact: true }).count(), 0);
          await page.goto(attributeEditor); await page.locator('.catalog-delete input[name=confirm]').check(); await submit(303, true);
          await page.goto(groupEditor); await page.locator('.catalog-delete input[name=confirm]').check(); await submit(303, true);
          assert.deepEqual(errors, []); results.push({ profile, width, js, locale, themes: ['default', 'essential'], passed: true }); console.log(suffix, 'passed');
        } finally { await context.close(); }
      }
    }
    await stop(); assert.equal(spawnSync(binary, ['--config', config, 'doctor'], { env, stdio: 'pipe' }).status, 0); await start(); assert.equal((await db.query('SELECT count(*)::int n FROM catalog_attributes')).rows[0].n, 0);
    fs.writeFileSync(path.join(output, 'results.json'), JSON.stringify(results, null, 2));
  } finally { await stop(); await browser.close(); await db.query('DROP SCHEMA IF EXISTS ' + schema + ' CASCADE'); await db.end(); fs.rmSync(folder, { recursive: true, force: true }); }
})().catch(error => { console.error(error); process.exitCode = 1; });
