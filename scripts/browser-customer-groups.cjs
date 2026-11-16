// Mandatory base customer groups: registration defaults and protected membership.
const assert = require('node:assert/strict'), fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { randomBytes } = require('node:crypto'), { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events'), { Client } = require('pg'), { chromium } = require('playwright');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test database required');
  const db = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL }); await db.connect();
  let browser;
  try { browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {}); } catch (error) { await db.end(); throw error; }
  const schema = 'talocart_customer_groups_' + process.pid + '_' + Date.now(), folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-customer-groups-'));
  const origin = 'http://127.0.0.1:3133', output = path.resolve('.local/verification/customer-groups/browser'); fs.mkdirSync(output, { recursive: true });
  const url = new URL(process.env.TALOCART_TEST_DATABASE_URL); url.searchParams.set('options', '-csearch_path=' + schema);
  const config = path.join(folder, 'talocart.toml'); fs.writeFileSync(config, '[server]\nlisten="127.0.0.1:3133"\n', { mode: 0o600 });
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
  const env = { ...process.env, TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: '127.0.0.1:3133', TALOCART_MEDIA_ROOT: path.join(folder, 'media'), TALOCART_LOG: 'warn' };
  let server; const results = [];
  const start = async () => {
    const fd = fs.openSync(path.join(output, 'server.log'), 'a', 0o600);
    server = spawn(binary, ['--config', config, 'serve'], { env, stdio: ['ignore', fd, fd] }); fs.closeSync(fd);
    for (let i = 0; i < 150; i++) { try { if ((await fetch(origin + '/health/ready')).ok) return; } catch {} await new Promise(r => setTimeout(r, 100)); }
    throw Error('Customer groups fixture readiness failed');
  };
  const stop = async () => { if (server && server.exitCode === null) { const done = once(server, 'exit'); server.kill('SIGTERM'); await done; } };
  try {
    await db.query('CREATE SCHEMA ' + schema); await db.query('SET search_path TO ' + schema);
    assert.equal(spawnSync(binary, ['--config', config, 'migrate'], { env, stdio: 'pipe' }).status, 0, 'Clean isolated migration'); await start();
    const setup = await browser.newContext(); const page = await setup.newPage(), password = randomBytes(20).toString('hex');
    await setup.addCookies([{ name: 'talocart_admin_language', value: 'it', url: origin }]); await page.goto(origin + '/setup');
    await page.getByLabel('Nome del negozio', { exact: true }).fill('Customer groups'); await page.getByLabel('Email amministratore').fill('owner@example.test'); await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByRole('button', { name: 'Crea negozio', exact: true }).click(); await page.waitForURL('**/admin/login');
    await page.getByLabel('Email amministratore').fill('owner@example.test'); await page.getByLabel('Password', { exact: true }).fill(password); await page.getByRole('button', { name: 'Accedi', exact: true }).click(); await page.waitForURL('**/admin');
    const cookies = (await setup.cookies()).filter(c => c.name === 'talocart_session');
    await page.goto(origin + '/admin/customer-groups/new');
    const csrf = await page.locator('main form [name=csrf]').first().inputValue();
    const languages = await setup.request.post(origin + '/admin/settings/storefront/languages', { data: new URLSearchParams({ csrf, primary_locale: 'en' }).toString() + '&locale=en&locale=it', headers: { 'content-type': 'application/x-www-form-urlencoded' }, maxRedirects: 0 }); assert.equal(languages.status(), 303);
    await db.query("INSERT INTO tax_rates(name,basis_points) VALUES('Quantity VAT',2200)");
    await db.query("INSERT INTO tax_classes(name,rate_id) SELECT 'Quantity class',id FROM tax_rates WHERE name='Quantity VAT'");
    const product=(await db.query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) SELECT 'Quantity product','QUANTITY','quantity-product',1000,9999,'published',id FROM tax_classes WHERE name='Quantity class' RETURNING id")).rows[0].id;
    await setup.close();
    for (const theme of ['default','essential']) for (const [profile, width, js] of [['desktop', 1440, true], ['mobile', 390, true], ['no-js', 1440, false], ['mobile-no-js', 390, false]]) for (const locale of ['en', 'it']) {
      await db.query('UPDATE storefront_settings SET theme_id=$1',[theme]); await db.query('UPDATE administrators SET locale=$1', [locale]);
      const context = await browser.newContext({ viewport: { width, height: 1000 }, javaScriptEnabled: js }); await context.addCookies(cookies);
      const page = await context.newPage(), suffix = theme + '-' + profile + '-' + locale, errors = []; page.on('pageerror', e => errors.push(e.message));
      const customerContext = await browser.newContext({ viewport: { width, height: 1000 }, javaScriptEnabled: js }); const buyer = await customerContext.newPage();
      const prefix = locale === 'it' ? '/it' : '';
      const submit = async (expected = 303, deletion = false) => {
        const done = page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname.startsWith('/admin/'));
        await page.locator(deletion ? 'main form[action$="/delete"] button' : (new URL(page.url()).pathname.startsWith('/admin/customers/') ? '#customer-profile-form button[value=save]' : 'main form:not([action$="/delete"]) button[value=save]')).click();
        assert.equal((await done).status(), expected); await page.waitForLoadState('load');
      };
      const customer = async email => {
        await buyer.goto(origin + prefix + '/account/register');
        for (const [name,value] of Object.entries({first_name:'Alex',last_name:suffix,email,password:'synthetic password',password_confirmation:'synthetic password'})) await buyer.locator('#customer-'+name).fill(value);
        await buyer.locator('#customer-form button[type=submit]:not([value=refresh])').click();await buyer.waitForURL(origin + prefix + '/account');
        const row = (await db.query('SELECT id,group_id FROM customers WHERE email=$1',[email])).rows[0];assert.ok(row);
        await buyer.locator('form[action$="/logout"] button').click();await buyer.waitForURL('**/account/login');return row;
      };
      const assign = async (id,group) => { await page.goto(origin+'/admin/customers/'+id);await page.locator('#customer-group').selectOption(String(group));await submit();assert.equal(await page.locator('#customer-group').inputValue(),String(group)); };
      const snapshot = async name => { assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),suffix+': no horizontal overflow');await page.screenshot({path:path.join(output,suffix+'-'+name+'.png'),fullPage:true});if(width===390&&name==='tiers-enabled'){await page.setViewportSize({width:320,height:1000});assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),suffix+': tiers fit 320px');await page.screenshot({path:path.join(output,suffix+'-tiers-320.png'),fullPage:true});await page.setViewportSize({width,height:1000});} };
      try {
        const previous = (await db.query('SELECT default_group_id FROM customer_group_configuration')).rows[0].default_group_id;
        const first = await customer('first-'+suffix+'@example.test');assert.equal(first.group_id,previous);
        await page.goto(origin+'/admin/customer-groups');assert.equal(await page.locator('h1').innerText(),locale==='it'?'Gruppi clienti':'Customer groups');
        await page.locator('main a[href$="/new"]').click();
        await page.locator('#cg-name_it').fill('Gruppo <safe> '+suffix);await page.locator('#cg-name_en').fill('Group <safe> '+suffix);
        await page.locator('#cg-name_en').focus();await page.keyboard.press('Tab');assert.equal(await page.locator('#cg-default').evaluate(e=>e===document.activeElement),true);assert.ok(await page.locator('#cg-default').evaluate(e=>getComputedStyle(e).outlineStyle!=='none'));
        await page.locator('main form').evaluate(f=>f.noValidate=true);await page.locator('#cg-name_en').fill('');await submit(422);
        assert.equal(await page.locator('#cg-name_en').getAttribute('aria-invalid'),'true');assert.equal(await page.locator('#cg-name_it').inputValue(),'Gruppo <safe> '+suffix);await snapshot('error');
        await page.locator('#cg-name_en').fill('Group <safe> '+suffix);await submit();
        assert.equal(await page.locator('main safe').count(),0);
        const group=(await db.query('SELECT id FROM customer_groups WHERE name_en=$1',['Group <safe> '+suffix])).rows[0].id;
        const editor=origin+'/admin/customer-groups/'+group;await page.goto(editor);
        const draft = async action => {
          const response=page.waitForResponse(r=>r.request().method()==='POST');
          await page.locator('button[value="'+action+'"]').click(); assert.equal((await response).status(),200);await page.waitForLoadState('load');
        };
        const beforeRevision=(await db.query('SELECT revision FROM customer_group_configuration')).rows[0].revision;
        await page.locator('#cg-tiers-enabled').check();await draft('add_tier');
        await page.locator('#cg-tier-quantity-0').fill('10');await page.locator('#cg-tier-percent-0').fill('5,5');await draft('add_tier');
        await page.locator('#cg-tier-quantity-1').fill('10');await page.locator('#cg-tier-percent-1').fill('10');await submit(422);
        assert.equal(await page.locator('#cg-tier-quantity-0').getAttribute('aria-invalid'),'true');assert.equal(await page.locator('#cg-tier-percent-0').inputValue(),'5,5');await snapshot('tiers-error');
        assert.equal((await db.query('SELECT revision FROM customer_group_configuration')).rows[0].revision,beforeRevision);
        await page.locator('#cg-tier-quantity-1').fill('50');await draft('add_tier');await draft('remove_tier_2');await submit();await page.goto(editor);
        assert.equal(await page.locator('#cg-tiers-enabled').isChecked(),true);assert.equal(await page.locator('#cg-tier-quantity-1').inputValue(),'50');await snapshot('tiers-enabled');
        const enter=page.waitForResponse(r=>r.request().method()==='POST');await page.locator('#cg-tier-percent-0').press('Enter');assert.equal((await enter).status(),303);await page.waitForLoadState('load');await page.goto(editor);assert.equal(await page.locator('input[name^=tier_quantity_]').count(),2,'Enter saves rather than removing a tier');
        await page.locator('#cg-tiers-enabled').uncheck();await submit();await page.goto(editor);
        assert.equal(await page.locator('#cg-tier-percent-0').inputValue(),'5.50');assert.equal(await page.locator('#cg-tiers-enabled').isChecked(),false);
        await page.locator('#cg-tiers-enabled').check();await submit();await page.goto(editor);
        const stale=await context.newPage();await stale.goto(editor);
        await page.locator('#cg-default').check();await submit();
        assert.equal((await db.query('SELECT group_id FROM customers WHERE id=$1',[first.id])).rows[0].group_id,previous,'Default change preserves existing customers');
        const pending=stale.waitForResponse(r=>r.request().method()==='POST');await stale.locator('#cg-name_en').fill('Pending '+suffix);await stale.locator('main form:not([action$="/delete"]) button[value=save]').click();assert.equal((await pending).status(),409);await stale.waitForLoadState('load');assert.equal(await stale.locator('#cg-name_en').inputValue(),'Pending '+suffix);await stale.close();
        const second=await customer('second-'+suffix+'@example.test');assert.equal(second.group_id,group,'Registration uses configured default');
        await page.goto(editor);assert.equal(await page.locator('#cg-default').isDisabled(),true);await snapshot('default');
        const anonymous=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
        try {
          const visitor=await anonymous.newPage();await visitor.goto(origin+prefix+'/products/quantity-product?quantity=10');
          const amount=n=>locale==='it'?n.replace('.',','):n;
          assert.ok((await visitor.locator('[data-product-price]').innerText()).includes(amount('10.00')));
          assert.equal(await visitor.locator('#cart-quantity').getAttribute('hx-post'),null);
          await page.goto(editor);assert.equal(await page.locator('#cg-tiers-guests').isChecked(),false);
          await page.locator('#cg-tiers-guests').focus();await page.keyboard.press('Space');assert.equal(await page.locator('#cg-tiers-guests').isChecked(),true);await submit();await page.goto(editor);
          assert.equal(await page.locator('#cg-tiers-guests').isChecked(),true);await snapshot('guest-option');
          await visitor.reload();assert.ok((await visitor.locator('[data-product-price]').innerText()).includes(amount('9.45')));
          if(js){const response=visitor.waitForResponse(r=>r.request().method()==='POST');await visitor.locator('#cart-quantity').fill('50');assert.equal((await response).status(),200);await visitor.waitForFunction(v=>document.querySelector('[data-product-price]').textContent.includes(v),amount('9.00'));}
          else {await visitor.locator('#cart-quantity').fill('50');await visitor.locator('.store-cart-add button[formaction]').click();await visitor.waitForLoadState('load');}
          assert.ok((await visitor.locator('[data-product-price]').innerText()).includes(amount('9.00')));
          await visitor.locator('.store-cart-add button[type=submit]:not([formaction])').click();await visitor.waitForURL('**/cart*');
          assert.ok((await visitor.locator('.cart-unit-price').innerText()).includes(amount('9.00')));
          assert.equal(await visitor.locator('[data-group-discount],.group-discount').count(),0);
          assert.ok(!(await anonymous.cookies()).some(c=>c.name==='talocart_customer'));
          await visitor.screenshot({path:path.join(output,suffix+'-guest-final-price.png'),fullPage:true});
          await page.goto(editor);await page.locator('#cg-tiers-guests').uncheck();await submit();await visitor.reload();
          assert.ok((await visitor.locator('.cart-unit-price').innerText()).includes(amount('10.00')));
          await page.goto(editor);await page.locator('#cg-tiers-guests').check();await page.locator('#cg-tiers-enabled').uncheck();await submit();await visitor.reload();
          assert.ok((await visitor.locator('.cart-unit-price').innerText()).includes(amount('10.00')));
          await page.goto(editor);await page.locator('#cg-tiers-enabled').check();await submit();await visitor.reload();
          assert.ok((await visitor.locator('.cart-unit-price').innerText()).includes(amount('9.00')));
        } finally {await anonymous.close();}
        await buyer.goto(origin+prefix+'/account/login');await buyer.locator('#customer-email').fill('second-'+suffix+'@example.test');await buyer.locator('#customer-password').fill('synthetic password');await buyer.locator('#customer-form button[type=submit]').click();await buyer.waitForURL(origin+prefix+'/account');
        await buyer.goto(origin+prefix+'/products/quantity-product');
        const number=value=>locale==='it'?value.replace('.',','):value;
        const preview=async quantity=>{
          if(js){const response=buyer.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname.endsWith('/products/quantity-product'));await buyer.locator('#cart-quantity').fill(String(quantity));assert.equal((await response).status(),200);await buyer.waitForFunction(value=>document.querySelector('[data-product-price]').textContent.includes(value),number(quantity>=50?'9.00':quantity>=10?'9.45':'10.00'));}
          else{await buyer.locator('#cart-quantity').fill(String(quantity));await buyer.locator('.store-cart-add button[formaction]').click();await buyer.waitForLoadState('load');}
          assert.ok((await buyer.locator('[data-product-price]').innerText()).includes(number(quantity>=50?'9.00':quantity>=10?'9.45':'10.00')));
          assert.equal(await buyer.locator('#cart-quantity').inputValue(),String(quantity));
        };
        await preview(10);await preview(1);await preview(10);
        if(js) {
          // Replacing the selection with the same value must still finish refresh.
          const unchanged=buyer.waitForResponse(r=>r.request().method()==='POST');
          await buyer.locator('#cart-quantity').dispatchEvent('input');
          assert.equal((await unchanged).status(),200);
          await buyer.waitForFunction(()=>!document.querySelector('.store-cart-add button[type=submit]').disabled);
          for(const failure of ['http','network']) {
            await buyer.route('**/products/quantity-product?*', async route => {
              if(route.request().method()!=='POST') return route.continue();
              if(failure==='network') return route.abort('failed');
              return route.fulfill({status:503,contentType:'text/plain',body:'Unavailable'});
            });
            await buyer.locator('#cart-quantity').fill('1');
            await buyer.locator('#product-price-error').waitFor({state:'visible'});
            assert.equal(await buyer.locator('#cart-quantity').inputValue(),'1');
            assert.equal(await buyer.locator('#product-unit-price').isVisible(),false);
            assert.equal(await buyer.locator('.store-cart-add button[type=submit]:not([formaction])').isEnabled(),false);
            assert.ok((await buyer.locator('#product-price-error').innerText()).includes(locale==='it'?'Impossibile aggiornare':'could not be updated'));
            await buyer.screenshot({path:path.join(output,suffix+'-price-'+failure+'.png'),fullPage:true});
            await buyer.unroute('**/products/quantity-product?*');
            const retried=buyer.waitForResponse(r=>r.request().method()==='POST');
            await buyer.locator('[data-price-retry]').click();assert.equal((await retried).status(),200);
            await buyer.waitForFunction(()=>!document.querySelector('.store-cart-add button[type=submit]').disabled);
            assert.equal(await buyer.locator('#product-price-error').isVisible(),false);
            assert.equal(await buyer.locator('#product-unit-price').isVisible(),true);
            assert.ok((await buyer.locator('[data-product-price]').innerText()).includes(number('10.00')));
            assert.equal(await buyer.locator('#cart-quantity').evaluate(e=>e===document.activeElement),true);
            await preview(10);
          }
        }

        assert.ok((await buyer.locator('#product-unit-net').innerText()).includes(number('7.75')));assert.ok((await buyer.locator('#product-unit-tax').innerText()).includes(number('1.70')));await buyer.screenshot({path:path.join(output,suffix+'-quantity-price.png'),fullPage:true});
        if(js) assert.equal(await buyer.locator('#cart-quantity').evaluate(e=>e===document.activeElement),true,'Price refresh keeps quantity focus');
        await buyer.locator('.store-cart-add button[type=submit]:not([formaction])').click();await buyer.waitForURL('**/cart*');
        assert.ok((await buyer.locator('.cart-unit-price').innerText()).includes(number('9.45')));
        const update=async quantity=>{await buyer.locator('.cart-quantity-form input[name=quantity]').fill(String(quantity));await buyer.locator('.cart-quantity-form button[value=update]').click();await buyer.waitForLoadState('load');};
        await update(9);assert.ok((await buyer.locator('.cart-unit-price').innerText()).includes(number('10.00')));
        await update(50);assert.ok((await buyer.locator('.cart-unit-price').innerText()).includes(number('9.00')));
        assert.equal(await buyer.locator('[data-group-discount],.group-discount').count(),0);
        assert.ok(!(await buyer.locator('main').innerText()).includes(locale==='it'?'Sconto quantità del gruppo':'Group quantity discount'));
        assert.ok(await buyer.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));await buyer.screenshot({path:path.join(output,suffix+'-cart-tiers.png'),fullPage:true});
        const other=(await db.query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) SELECT 'Other','OTHER-'||upper($2),'other-'||$2,1000,100,'published',tax_class_id FROM catalog_products WHERE id=$1 RETURNING id",[product,suffix])).rows[0].id;
        await buyer.goto(origin+prefix+'/products/other-'+suffix);
        await buyer.locator('.store-cart-add button[type=submit]:not([formaction])').click();await buyer.waitForURL('**/cart*');
        for(const unavailable of [false,true]) {
          await db.query(unavailable?"UPDATE catalog_products SET status='draft' WHERE id=$1":"UPDATE catalog_products SET stock=0 WHERE id=$1",[other]);
          await buyer.reload();
          assert.ok((await buyer.locator('.cart-unit-price').first().innerText()).includes(number('9.00')), 'Unrelated invalid row preserves tier price');
        }
        await db.query("UPDATE catalog_products SET stock=100,status='published' WHERE id=$1",[other]);
        await buyer.reload();assert.ok((await buyer.locator('.cart-unit-price').first().innerText()).includes(number('9.00')));
        await assign(first.id,group);await snapshot('assignment');
        await page.locator('#customer-group').evaluate(e=>{e.add(new Option('Malformed','bad',true,true));});await submit(422);
        assert.equal(await page.locator('#customer-group').getAttribute('aria-invalid'),'true');assert.equal(await page.locator('#customer-group-error').count(),1);await snapshot('assignment-error');
        await page.locator('#customer-group').selectOption(String(group));await submit();
        await page.goto(origin+'/admin/customer-groups/'+previous);await page.locator('#cg-default').check();await submit();
        await page.goto(editor);await page.locator('#cg-confirm').check();await submit(409,true);assert.ok((await page.locator('#cg-error').innerText()).includes(locale==='it'?'non può essere eliminato':'cannot be deleted'));await snapshot('protected');
        await assign(first.id,previous);await assign(second.id,previous);
        await page.goto(editor);await page.locator('#cg-confirm').check();await submit(303,true);
        assert.equal((await db.query('SELECT count(*)::int AS count FROM customer_groups WHERE id=$1',[group])).rows[0].count,0);
        await page.goto(origin+'/admin/customers');await snapshot('customers');assert.deepEqual(errors,[]);results.push({theme,profile,locale,javascript:js,passed:true});
      } finally { await customerContext.close();await context.close(); }
    }
    await stop();await start();
    assert.equal((await db.query('SELECT count(*)::int AS count FROM customers c JOIN customer_groups g ON g.id=c.group_id')).rows[0].count,32,'Membership survives restart');
    assert.equal((await fetch(origin+'/health/ready')).status,200);
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));console.log(JSON.stringify(results));
  } finally {
    await stop();await browser.close();await db.query('DROP SCHEMA IF EXISTS '+schema+' CASCADE');await db.end();fs.rmSync(folder,{recursive:true,force:true});
  }
})().catch(e=>{console.error(e.stack);process.exit(1);});
