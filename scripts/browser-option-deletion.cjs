// Synthetic option deletion: contextual feedback, paging, keyboard, EN/IT, JS/no-JS.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { Client } = require('pg');
const { chromium } = require('playwright');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { randomBytes } = require('node:crypto');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL);
  const db = new Client({connectionString: process.env.TALOCART_TEST_DATABASE_URL});
  await db.connect();
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? {channel: process.env.TALOCART_BROWSER_CHANNEL} : {});
  const schema = `talocart_delete_audit_${process.pid}_${Date.now()}`;
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-delete-audit-'));
  const output = path.resolve('.local/verification/option-deletion');
  fs.mkdirSync(output, {recursive: true});
  const origin = 'http://127.0.0.1:3197';
  const config = path.join(folder, 'talocart.toml');
  fs.writeFileSync(config, '[server]\nlisten="127.0.0.1:3197"\n', {mode: 0o600});
  const url = new URL(process.env.TALOCART_TEST_DATABASE_URL);
  url.searchParams.set('options', `-csearch_path=${schema}`);
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/debug/talocart');
  const env = {...process.env, TALOCART_DATABASE_URL:url.href, TALOCART_LISTEN:'127.0.0.1:3197', TALOCART_MEDIA_ROOT:path.join(folder,'media'), TALOCART_LOG:'warn'};
  const results = [];
  let server;
  try {
    await db.query(`CREATE SCHEMA ${schema}`);
    await db.query(`SET search_path TO ${schema}`);
    assert.equal(spawnSync(binary, ['--config', config, 'migrate'], {env, stdio:'pipe'}).status, 0);
    server = spawn(binary, ['--config',config,'serve'], {env,stdio:'ignore'});
    let ready = false;
    for(let i=0;i<150;i++) { try { if((await fetch(origin+'/health/ready')).ok) {ready=true;break;} } catch {} await new Promise(r=>setTimeout(r,100)); }
    assert.ok(ready);
    const setup = await browser.newContext();
    await setup.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);
    const page = await setup.newPage();
    const password = randomBytes(20).toString('hex');
    await page.goto(origin+'/setup');
    await page.getByLabel('Nome del negozio',{exact:true}).fill('Delete audit');
    await page.getByLabel('Email amministratore').fill('owner@example.test');
    await page.getByLabel('Password',{exact:true}).fill(password);
    await page.getByLabel('Installa dati demo',{exact:true}).check();
    await page.getByRole('button',{name:'Crea negozio',exact:true}).click();
    await page.waitForURL('**/admin/login');
    await page.getByLabel('Email amministratore').fill('owner@example.test');
    await page.getByLabel('Password',{exact:true}).fill(password);
    await page.getByRole('button',{name:'Accedi',exact:true}).click();
    await page.waitForURL('**/admin');
    const cookies = (await setup.cookies()).filter(c=>c.name==='talocart_session');
    for (const locale of ['it', 'en']) for (const [profile, width, js] of [['desktop', 1440, true], ['mobile', 390, true], ['no-js', 1440, false], ['mobile-no-js', 390, false]]) {
      await db.query('UPDATE administrators SET locale=$1', [locale]);
      const option = (await db.query("INSERT INTO catalog_options(name) VALUES('Audit Color') RETURNING id")).rows[0].id;
      const ids = [];
      const usedName = 'Audit <value> & 20';
      for (let n = 0; n < 55; n++) ids.push((await db.query('INSERT INTO catalog_option_values(option_id,name,sort_order) VALUES($1,$2,$3) RETURNING id', [option, n === 50 ? usedName : `Audit value ${String(n).padStart(2, '0')}`, n])).rows[0].id);
      const product = (await db.query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,category_id,brand_id,tax_class_id) SELECT 'Audit product','AUDITDELETE','audit-delete',100,10,'published',category_id,brand_id,tax_class_id FROM catalog_products WHERE model='MUG' RETURNING id")).rows[0].id;
      await db.query('INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2)', [product, option]);
      await db.query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],'AUDIT-USED',100,10)", [product, ids[50]]);
      const context = await browser.newContext({ viewport: { width, height: 1000 }, javaScriptEnabled: js });
      await context.addCookies(cookies);
      const p = await context.newPage();
      const pageErrors = [];
      p.on('pageerror', e => pageErrors.push(e.message));
      const editor = `${origin}/admin/catalog/options/${option}`;
      const endpoint = id => `${editor}/values/${id}/delete`;
      const deletion = id => p.locator(`form[action="/admin/catalog/options/${option}/values/${id}/delete"]`);
      // The confirmation panel is a native disclosure: open it from the keyboard like a user would.
      const reveal = async id => { await deletion(id).locator('xpath=ancestor::details[1]/summary').press('Enter'); assert.ok(await deletion(id).isVisible()); };
      try {
        await p.goto(editor + '?page=2');
        assert.equal(await p.locator('html').getAttribute('lang'), locale);
        assert.equal(await deletion(ids[50]).locator('input[name=page]').inputValue(), '2');
        // Required native confirmation blocks an accidental keyboard submission.
        assert.equal(await deletion(ids[50]).isVisible(), false);
        await reveal(ids[50]);
        await deletion(ids[50]).locator('button').press('Enter');
        assert.equal(new URL(p.url()).search, '?page=2');
        assert.equal(await deletion(ids[50]).locator('input[name=confirm]').evaluate(el => el.validity.valueMissing), true);
        await deletion(ids[50]).locator('input[name=confirm]').press('Space');
        const [failed] = await Promise.all([p.waitForNavigation(), deletion(ids[50]).locator('button').press('Enter')]);
        assert.equal(failed.status(), 409);
        assert.equal(await p.locator('#option-name').getAttribute('aria-invalid'), null);
        assert.equal(await p.locator('.option-value-row').count(), 5);
        const error = deletion(ids[50]).getByRole('alert');
        const message = await error.innerText();
        assert.ok(message.includes(usedName));
        assert.ok(message.includes(locale === 'it' ? 'è utilizzato da una o più varianti' : 'is used by one or more variants'));
        assert.equal(await error.getAttribute('id'), 'option-value-delete-error');
        assert.equal(await deletion(ids[50]).locator('button').getAttribute('aria-describedby'), 'option-value-delete-error');
        assert.ok(await error.evaluate(el => el === document.activeElement), 'Error receives native focus even without JavaScript');
        assert.ok(await p.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
        await p.screenshot({ path: path.join(output, `${locale}-${profile}-blocked.png`) });
        const csrf = await p.locator('input[name=csrf]').first().inputValue();
        const missingConfirm = await context.request.post(endpoint(ids[50]), { form: { csrf, page: '2' } });
        assert.equal(missingConfirm.status(), 422);
        const missingBody = await missingConfirm.text();
        assert.ok(missingBody.includes(locale === 'it' ? 'Conferma l’eliminazione del valore' : 'Confirm deletion of the value'));
        assert.ok(missingBody.includes('id="option-value-delete-error"'));
        assert.equal((await context.request.post(endpoint(ids[50]), { form: { csrf: 'wrong', confirm: 'true' } })).status(), 403);
        assert.equal((await context.request.get(endpoint(ids[50]))).status(), 405);
        assert.equal((await db.query('SELECT count(*)::int AS count FROM catalog_option_values WHERE id=$1', [ids[50]])).rows[0].count, 1);
        // A successful removal keeps page two, with a deletion-specific status.
        await p.goto(editor + '?page=2');
        await reveal(ids[54]);
        await deletion(ids[54]).locator('input[name=confirm]').press('Space');
        await Promise.all([p.waitForNavigation(), deletion(ids[54]).locator('button').press('Enter')]);
        assert.equal(new URL(p.url()).search, '?deleted=1&page=2');
        assert.equal(await p.getByRole('status').innerText(), locale === 'it' ? 'Valore eliminato.' : 'Value deleted.');
        assert.equal((await db.query('SELECT count(*)::int AS count FROM catalog_option_values WHERE id=$1', [ids[54]])).rows[0].count, 0);
        // Remove other second-page values, then the last one through its real form.
        await db.query('DELETE FROM catalog_variants WHERE product_id=$1', [product]);
        for (const id of ids.slice(51, 54)) assert.equal((await context.request.post(endpoint(id), { form: { csrf, confirm: 'true', page: '2' }, maxRedirects: 0 })).status(), 303);
        await p.goto(editor + '?page=2');
        assert.equal(await p.locator('.option-value-row').count(), 1);
        await reveal(ids[50]);
        await deletion(ids[50]).locator('input[name=confirm]').press('Space');
        await Promise.all([p.waitForNavigation(), deletion(ids[50]).locator('button').press('Enter')]);
        assert.equal(new URL(p.url()).search, '?deleted=1&page=1');
        assert.equal(await p.locator('.option-value-row').count(), 50);
        const stale = await context.request.post(endpoint(ids[50]), { form: { csrf, confirm: 'true', page: '2' } });
        assert.equal(stale.status(), 404);
        const staleBody = await stale.text();
        assert.ok(staleBody.includes(locale === 'it' ? 'Il valore non è più presente' : 'The value is no longer present'));
        assert.ok(!/id="option-name"[^>]*aria-invalid/.test(staleBody));
        assert.deepEqual(pageErrors, []);
        results.push(`${locale}-${profile}: keyboard/native confirmation, inline named error/focus, 409/422/403/405/404, retained page, deletion status, empty-page fallback and escaping OK`);
      } finally {
        await context.close();
        await db.query('DELETE FROM catalog_products WHERE id=$1', [product]);
        await db.query('DELETE FROM catalog_option_values WHERE option_id=$1', [option]);
        await db.query('DELETE FROM catalog_options WHERE id=$1', [option]);
      }
    }
    fs.writeFileSync(path.join(output, 'results.json'), JSON.stringify(results, null, 2));
    console.log(results.join('\n'));
  } finally {
    await browser.close();
    if (server && server.exitCode === null) { server.kill('SIGTERM'); await once(server, 'exit'); }
    await db.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);
    await db.end();
    fs.rmSync(folder, { recursive: true, force: true });
  }
})().catch(e => { console.error(e); process.exitCode = 1; });
