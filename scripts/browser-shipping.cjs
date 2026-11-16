// Real HTTP forms, isolated database schemas; all profiles cover both interface languages.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { Client } = require('pg');
const { chromium } = require('playwright');
const orderStatuses = require('./browser-order-statuses.cjs');
const chargeTaxes = require('./browser-charge-taxes.cjs');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test database required');
  const db = new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();
  const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
  const output=path.resolve('.local/verification/shipping');fs.mkdirSync(output,{recursive:true});
  const results=[];
  try { for(const [name,width,js] of [['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false],['desktop-demo',1440,true]]) {
    const schema=`talocart_shipping_browser_${process.pid}_${Date.now()}`;
    const folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-shipping-'));
    const origin='http://127.0.0.1:3109';const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options',`-csearch_path=${schema}`);
    const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3109"\n',{mode:0o600});
    const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3109',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
    const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart');let server;
    const cli=command=>{const r=spawnSync(binary,['--config',config,command],{env,encoding:'utf8'});assert.equal(r.status,0,`CLI ${command} failed (private output omitted)`);};
    const start=async()=>{server=spawn(binary,['--config',config,'serve'],{env,stdio:'ignore'});for(let i=0;i<100;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Readiness failed');};
    const stop=async()=>{if(server&&server.exitCode===null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
    const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
    try {
      await db.query(`CREATE SCHEMA ${schema}`);await db.query(`SET search_path TO ${schema}`);cli('migrate');assert.deepEqual((await db.query("SELECT code,active FROM shipping_methods ORDER BY code")).rows,[{code:'free_shipping',active:false},{code:'national_express',active:false}]);await start();
      const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
      await context.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);await page.goto(origin+'/setup');const password=randomBytes(20).toString('hex');
      await page.getByLabel('Nome del negozio',{exact:true}).fill('Shipping methods');await page.getByLabel('Nome visualizzato',{exact:true}).fill('Test owner');await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);if(name==='desktop-demo')await page.locator('#install-demo').check();await page.getByRole('button',{name:'Crea negozio',exact:true}).click();await page.waitForURL('**/admin/login');
      await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);await page.getByRole('button',{name:'Accedi',exact:true}).click();await page.waitForURL('**/admin');
      assert.equal(Number((await db.query('SELECT count(*) FROM catalog_products')).rows[0].count)>0,name==='desktop-demo');
      const classId=await chargeTaxes.fixture(db);
      const save=async(status=303)=>{const response=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().includes('/settings/shipping/'));await page.locator('main button[type=submit]').click();assert.equal((await response).status(),status);if(status===303)await page.waitForURL('**/admin/settings/shipping');};
      const configureCountries=async(codes)=>{
        await page.goto(origin+'/admin/settings/countries');
        await page.locator('#checkout-countries').selectOption(codes);
        await page.locator('#settings-save').click();await page.waitForURL('**/admin/settings/countries?saved=1');
      };
      for(const lang of ['it','en']) {
        await db.query('UPDATE administrators SET locale=$1',[lang]);
        await page.goto(origin+'/admin/settings/shipping');
        assert.equal(await page.locator('h1').innerText(),lang==='it'?'Metodi di spedizione':'Shipping methods');
        assert.equal(await page.locator('.sidebar-submenu [aria-current=page]').count(),1);
        assert.equal(await page.locator('main .catalog-row').count(),2);
        assert.equal(await page.locator('main a[href$="/new"]').count(),0);
        // Brand-new installs, with or without demo, have both methods and no implicit destinations.
        if(lang==='it') {
          await page.goto(origin+'/admin/settings/shipping/1');
          assert.equal(await page.locator('input[name=countries]').count(),0);
          await page.locator('#shipping-active').check();await save(422);
          assert.equal(await page.locator('#shipping-active').isChecked(),true);
        }
        await configureCountries(['IT','FR','DE']);
        for(const [id,free] of [[1,false],[2,true]]) {
          await page.goto(origin+`/admin/settings/shipping/${id}`);
          await orderStatuses.checkReturnLink(page,'/admin/settings/shipping');
          assert.equal(await page.locator('#shipping-fee-kind').count(),free?0:1);
          assert.equal(await page.locator('input[name=countries]').count(),3);
          for(const code of ['IT','FR','DE']) assert.ok(await page.locator(`#shipping-country-${code}`).evaluate(e=>{const a=e.getBoundingClientRect(),b=e.closest('label').getBoundingClientRect();return Math.abs(a.top+a.height/2-b.top-b.height/2)<4;}),'Country checkbox aligns with its label');
          await page.locator('#shipping-country-IT').check();await page.locator('#shipping-country-FR').check();await page.locator('#shipping-country-DE').uncheck();
          await page.locator('#shipping-active').check();
          await page.locator('#shipping-delivery_days').fill('-1');await save(422);
          assert.equal(await page.locator('#shipping-delivery_days').getAttribute('aria-invalid'),'true');
          assert.equal(await page.locator('#shipping-country-FR').isChecked(),true);
          await page.locator('#shipping-delivery_days').fill('3');
          if(!free) {
            await page.locator('#shipping-fee_value').fill('5.90');await chargeTaxes.missingClass({page,save,prefix:'shipping',lang,classId});
            await page.locator('#shipping-fee-kind').selectOption('percent');await page.locator('#shipping-fee_value').fill('100.01');await save(422);
            assert.equal(await page.locator('#shipping-fee_value').getAttribute('aria-invalid'),'true');
            await page.locator('#shipping-fee_value').fill('2,50');await page.locator('#shipping-free_from').fill('100');
          }
          await page.locator('#shipping-instructions_it').fill('<script>alert(1)</script>');await save();
          await page.goto(origin+`/admin/settings/shipping/${id}`);
          assert.equal(await page.locator('#shipping-delivery_days').inputValue(),'3');
          assert.equal(await page.locator('#shipping-country-IT').isChecked(),true);assert.equal(await page.locator('#shipping-country-FR').isChecked(),true);
          assert.equal(await page.locator('#shipping-instructions_it').inputValue(),'<script>alert(1)</script>');
          await page.locator('#shipping-name_it').focus();await page.keyboard.press('Tab');assert.equal(await page.locator('#shipping-name_en').evaluate(e=>e===document.activeElement),true);
          assert.ok(await page.locator('#shipping-name_en').evaluate(e=>getComputedStyle(e).outlineStyle!=='none'));
          assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
          await page.screenshot({path:path.join(output,`${name}-${lang}-${id}-editor.png`),fullPage:true});
          if(!free) await chargeTaxes.mode({page,db,save,prefix:'shipping',lang});
          if(!free) {await page.locator('#shipping-fee-kind').selectOption('fixed');await page.locator('#shipping-fee_value').fill('5.90');await page.locator('#shipping-free_from').fill('');}
          await page.locator('#shipping-delivery_days').fill('');await save();
          const row=(await db.query('SELECT fee_kind,fee_value,free_from_cents,delivery_days FROM shipping_methods WHERE id=$1',[id])).rows[0];assert.deepEqual(row,{fee_kind:'fixed',fee_value:free?'0':'590',free_from_cents:'0',delivery_days:null});
          await page.goto(origin+`/admin/settings/shipping/${id}`);await db.query('UPDATE shipping_configuration SET revision=revision+1');await page.locator('#shipping-delivery_days').fill('5');await save(409);assert.equal(await page.locator('#shipping-delivery_days').inputValue(),'5');assert.equal(await page.locator('#shipping-country-FR').isChecked(),true);
        }
        await page.goto(origin+'/admin/settings/shipping');assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));await page.screenshot({path:path.join(output,`${name}-${lang}-list.png`),fullPage:true});
        // Keeping destinations must retain assignments. Removing one clears only that assignment.
        await configureCountries(['IT','FR','DE']);assert.equal((await db.query('SELECT count(*) FROM shipping_method_countries')).rows[0].count,'4');
        await configureCountries(['IT','DE']);assert.equal((await db.query('SELECT count(*) FROM shipping_method_countries')).rows[0].count,'2');
        await configureCountries(['IT','FR','DE']);await page.goto(origin+'/admin/settings/shipping/1');assert.equal(await page.locator('#shipping-country-FR').isChecked(),false);
      }
      await stop();cli('doctor');await start();
      for(const id of [1,2]) {await page.goto(origin+`/admin/settings/shipping/${id}`);assert.equal(await page.locator('#shipping-country-IT').isChecked(),true);assert.equal(await page.locator('#shipping-active').isChecked(),true);assert.equal(await page.locator('#shipping-delivery_days').inputValue(),'');}
      assert.deepEqual(errors,[]);results.push({name,languages:['it','en'],javascript:js,passed:true});console.log(name,'passed');
    } finally {await stop();await context.close();await db.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);fs.rmSync(folder,{recursive:true,force:true});}
  }} finally {await browser.close();await db.end();}
  fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
})().catch(e=>{console.error(e);process.exitCode=1;});
