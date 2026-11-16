// Currency and country-zone UI: every profile owns an isolated schema and temporary media.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { Client } = require('pg');
const { chromium } = require('playwright');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test database required');
  const db = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL }); await db.connect();
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {});
  const output = path.resolve('.local/verification/currency-zones'); fs.mkdirSync(output, { recursive: true });
  const profiles = [['desktop',1440,true], ['mobile',390,true], ['no-js',1440,false], ['mobile-no-js',390,false]];
  const results = [];
  try {
    for (const [name,width,js] of profiles.filter(([name]) => !process.env.TALOCART_CURRENCY_TEST_CASE || process.env.TALOCART_CURRENCY_TEST_CASE===name)) {
      const schema = `talocart_currency_browser_${process.pid}_${Date.now()}`;
      const folder = fs.mkdtempSync(path.join(os.tmpdir(),'talocart-currency-'));
      const port = Number(process.env.TALOCART_CURRENCY_TEST_PORT || 3105), origin=`http://127.0.0.1:${port}`;
      const url = new URL(process.env.TALOCART_TEST_DATABASE_URL); url.searchParams.set('options',`-csearch_path=${schema}`);
      const config=path.join(folder,'talocart.toml'); fs.writeFileSync(config,`[server]\nlisten="127.0.0.1:${port}"\n`,{mode:0o600});
      const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:`127.0.0.1:${port}`,TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
      const binary=path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
      const cli=command=>{const r=spawnSync(binary,['--config',config,command],{env,encoding:'utf8'}); assert.equal(r.status,0,`CLI ${command} failed (private output omitted)`);};
      const query=async(sql,args)=>{await db.query(`SET search_path TO ${schema}`);return db.query(sql,args);};
      let server; const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
      context.on('page', p => p.on('dialog', d => d.accept()));
      const start=async()=>{
        server=spawn(binary,['--config',config,'serve'],{env,stdio:'ignore'});
        let ready=false;
        for(let i=0;i<100;i++){try{if((await fetch(origin+'/health/ready')).ok){ready=true;break;}}catch{} await new Promise(r=>setTimeout(r,100));}
        assert.ok(ready,'Server readiness');
      };
      const stop=async()=>{if(server && server.exitCode===null){const exited=once(server,'exit');server.kill('SIGTERM');await exited;}};
      try {
        await db.query(`CREATE SCHEMA ${schema}`);cli('migrate');cli('migrate');await start();
        const page=await context.newPage(), errors=[];page.on('pageerror',e=>errors.push(e.message));
        await context.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);
        await page.goto(origin+'/setup');
        const password=randomBytes(20).toString('hex');
        await page.getByLabel('Nome del negozio',{exact:true}).fill('Currency zones');await page.getByLabel('Nome visualizzato',{exact:true}).fill('Test owner');
        await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);
        await page.getByRole('button',{name:'Crea negozio',exact:true}).click();await page.waitForURL('**/admin/login');
        await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);await page.getByRole('button',{name:'Accedi',exact:true}).click();await page.waitForURL('**/admin');
        // Replace the installed Italy zone through the native form before the custom-zone scenario.
        await page.goto(origin+'/admin/settings/taxes/zones');
        await page.getByRole('link',{name:'Italia / Italy',exact:true}).click();
        const installedZoneDelete=page.locator('form[action$="/delete"]');
        await installedZoneDelete.locator('[name=confirm]').check();await installedZoneDelete.locator('button').click();
        await page.waitForURL('**/admin/settings/taxes/zones');
        const base=(await query("INSERT INTO tax_rates(name,basis_points) VALUES('Base 50',5000) RETURNING id")).rows[0].id;
        const local=(await query("INSERT INTO tax_rates(name,basis_points) VALUES('Local 20',2000) RETURNING id")).rows[0].id;
        const baseClass=(await query("INSERT INTO tax_classes(name,rate_id) VALUES('Base class',$1) RETURNING id",[base])).rows[0].id;
        const localClass=(await query("INSERT INTO tax_classes(name,rate_id) VALUES('Zone class',$1) RETURNING id",[local])).rows[0].id;
        const inactiveClass=(await query("INSERT INTO tax_classes(name,rate_id,active) VALUES('Inactive class',$1,false) RETURNING id",[local])).rows[0].id;
        const navigate=async route=>{await page.goto(origin+route);assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),route+' fits viewport');};
        const savePrefs=async()=>{await page.locator('#settings-save').click();await page.waitForURL('**/admin/settings/taxes?saved=1');};
        const saveZone=async()=>{await page.locator('form:has(#zone-name) button[type=submit]').click();await page.waitForURL(/\/admin\/settings\/taxes\/zones\/\d+\?saved=1$/);};
        await navigate('/admin/settings/taxes');assert.equal(await page.locator('#store-currency').inputValue(),'EUR');assert.equal(await page.locator('#store-currency option').count(),13);
        await page.locator('#store-currency').selectOption('USD');await page.locator('#store-tax-country').selectOption('IT');await page.locator('#price-mode').selectOption('exclusive');await page.locator('#default-class').selectOption(baseClass);await savePrefs();
        assert.deepEqual((await query("SELECT s.currency,p.country_code,p.price_mode FROM store_settings s CROSS JOIN pricing_settings p")).rows[0],{currency:'USD',country_code:'IT',price_mode:'exclusive'});
        for(const language of ['it','en']) {
          await navigate('/admin/account');const langForm=page.locator('form:has(select[name="locale"])');await langForm.locator('select').selectOption(language);await langForm.locator('button[type=submit]').click();await page.waitForURL('**/admin/account?saved=1');
          await navigate('/admin/settings/taxes/zones');await page.getByRole('link',{name:language==='it'?'Nuova zona fiscale':'New tax zone',exact:false}).click();
          assert.equal(await page.locator(`#zone-rule-${inactiveClass} option[value="${local}"]`).isDisabled(),false,'Inactive class remains selectable when configuring an inactive zone without JavaScript');
          await page.locator('#zone-name').fill('Inactive zone');await page.locator(`#zone-rule-${inactiveClass}`).selectOption(local);await page.locator('#zone-countries').selectOption('US');
          const inactiveRejected=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().endsWith('/zones/new'));await page.locator('form:has(#zone-name) button[type=submit]').click();assert.equal((await inactiveRejected).status(),409);assert.equal(await page.locator(`#zone-rule-${inactiveClass}`).inputValue(),local);
          await page.locator('input[name=active]').uncheck();await saveZone();assert.equal((await query("SELECT active FROM tax_zones WHERE name='Inactive zone'")).rows[0].active,false);
          const inactiveDelete=page.locator('form[action$="/delete"]');await inactiveDelete.locator('[name=confirm]').check();await inactiveDelete.locator('button').click();await page.waitForURL('**/admin/settings/taxes/zones');await navigate('/admin/settings/taxes/zones/new');
          await page.locator('#zone-name').fill(`Europe ${language}`);await page.locator('#zone-description').fill('Preserved description');await page.locator(`#zone-rule-${baseClass}`).selectOption(local);await page.locator('#zone-countries').selectOption(['IT','FR']);
          assert.equal(await page.locator('#zone-countries option').count(),249);await page.locator('#zone-name').focus();await page.keyboard.press('Tab');assert.equal(await page.locator('#zone-description').evaluate(e=>e===document.activeElement),true);
          await saveZone();const zoneId=(await query('SELECT id FROM tax_zones')).rows[0].id;
          assert.equal((await query('SELECT count(*) AS count FROM tax_zone_countries')).rows[0].count,'2');
          assert.equal(await page.locator('input[name=active]').evaluate(e=>getComputedStyle(e.closest('label')).display),'flex','Active zone control stays beside its label');
          await page.screenshot({path:path.join(output,`zone-${name}-${language}.png`),fullPage:true});
          // Native multiple selection, update and validation retain the complete form.
          await page.locator('#zone-countries').selectOption(['IT','DE']);await saveZone();
          assert.deepEqual((await query('SELECT country_code FROM tax_zone_countries ORDER BY country_code')).rows.map(r=>r.country_code),['DE','IT']);
          await navigate('/admin/settings/taxes/zones/new');await page.locator('#zone-name').fill('Conflict');await page.locator('#zone-description').fill('Keep this');await page.locator(`#zone-rule-${baseClass}`).selectOption(local);await page.locator('#zone-countries').selectOption(['IT','US']);
          const conflict=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().endsWith('/zones/new'));await page.locator('form:has(#zone-name) button[type=submit]').click();assert.equal((await conflict).status(),409);await page.getByRole('alert').waitFor();
          assert.equal(await page.locator('#zone-description').inputValue(),'Keep this');assert.deepEqual(await page.locator('#zone-countries').evaluate(e=>[...e.selectedOptions].map(o=>o.value).sort()),['IT','US']);assert.equal(await page.locator('#zone-countries').getAttribute('aria-invalid'),'true');
          await navigate('/admin/settings/taxes/preview');await page.locator('#preview-amount').fill('100');await page.locator('#preview-class').selectOption(baseClass);await page.locator('#preview-country').selectOption('IT');await page.locator('form[action$="/preview"] button').click();await page.locator('.tax-result').waitFor();
          assert.equal(await page.locator('[data-total=gross]').innerText(),language==='it'?'120,00 USD':'120.00 USD');assert.ok((await page.locator('[data-tax-zone]').innerText()).includes(`Europe ${language}`));
          await page.locator('#preview-country').selectOption('FR');await page.locator('form[action$="/preview"] button').click();await page.locator('.tax-result').waitFor();assert.equal(await page.locator('[data-total=gross]').innerText(),language==='it'?'150,00 USD':'150.00 USD');
          await page.screenshot({path:path.join(output,`preferences-${name}-${language}.png`),fullPage:true});
          // A destination can be tested without mutating the catalog country.
          assert.equal((await query('SELECT country_code FROM pricing_settings')).rows[0].country_code,'IT');
          await navigate(`/admin/settings/taxes/zones/${zoneId}`);const deletion=page.locator('form[action$="/delete"]');await deletion.locator('[name=confirm]').check();await deletion.locator('button').click();await page.waitForURL('**/admin/settings/taxes/zones');assert.equal((await query('SELECT count(*) FROM tax_zones')).rows[0].count,'0');
        }
        // Persistent configured prices across the public surface, variants and real restart.
        await navigate('/admin/settings/taxes/zones/new');await page.locator('#zone-name').fill('Final zone');await page.locator(`#zone-rule-${baseClass}`).selectOption(local);await page.locator('#zone-countries').selectOption('IT');await saveZone();
        const product=(await query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Synthetic price','USD-TEST','usd-test',10000,10,'published',$1) RETURNING id",[baseClass])).rows[0].id;
        const axis=(await query("INSERT INTO catalog_options(name) VALUES('Size') RETURNING id")).rows[0].id;
        const value=(await query("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Large') RETURNING id",[axis])).rows[0].id;
        await query('INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2)',[product,axis]);await query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2]::bigint[],'USD-VAR',20000,10)",[product,value]);
        for(const [route,amount] of [['/catalog','240,00 USD'],['/en/catalog','240.00 USD'],[`/products/usd-test?value_1=${value}`,'240,00 USD'],[`/en/products/usd-test?value_1=${value}`,'240.00 USD'],[`/admin/catalog/products/${product}/variants`,'200.00 USD']]) {
          await navigate(route);assert.ok((await page.locator('main').innerText()).includes(amount),route);assert.ok(!(await page.locator('main').innerText()).includes('€'),route);
        }
        await navigate('/admin/settings/taxes');await page.locator('#store-currency').selectOption('GBP');await page.locator('#store-tax-country').selectOption('FR');
        const blocked=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().endsWith('/admin/settings/save'));await page.locator('#settings-save').click();assert.equal((await blocked).status(),409);await page.getByRole('alert').waitFor();
        assert.equal(await page.locator('#store-currency').inputValue(),'GBP');assert.equal(await page.locator('#store-currency option').count(),13,'A valid rejected currency is retained without a duplicate option');assert.equal(await page.locator('#store-tax-country').inputValue(),'FR');assert.deepEqual((await query('SELECT s.currency,p.country_code FROM store_settings s CROSS JOIN pricing_settings p')).rows[0],{currency:'USD',country_code:'IT'});
        await require('./browser-checkout-countries.cjs')({page,query,origin,output,name,js});
        await stop();cli('doctor');await start();await navigate('/en/catalog');assert.ok((await page.locator('main').innerText()).includes('240.00 USD'));await navigate('/admin/settings/countries');assert.deepEqual(await page.locator('#checkout-countries').evaluate(e=>[...e.selectedOptions].map(o=>o.value).sort()),['DE','IT']);
        assert.deepEqual(errors,[]);results.push({name,languages:['it','en'],javascript:js,passed:true});
      } finally {
        await context.close();await stop();await db.query(`DROP SCHEMA ${schema} CASCADE`);fs.rmSync(folder,{recursive:true,force:true});
      }
    }
  } finally {await browser.close();await db.end();}
  fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));console.log(JSON.stringify(results));
})().catch(e=>{console.error(e.message);process.exitCode=1;});
