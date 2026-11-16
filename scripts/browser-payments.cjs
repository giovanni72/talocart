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
  const output=path.resolve('.local/verification/payments');fs.mkdirSync(output,{recursive:true});
  const results=[];
  try { for(const [name,width,js] of [['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false],['desktop-demo',1440,true]]) {
    const schema=`talocart_payment_browser_${process.pid}_${Date.now()}`;
    const folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-payment-'));
    const origin='http://127.0.0.1:3108';const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options',`-csearch_path=${schema}`);
    const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3108"\n',{mode:0o600});
    const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3108',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
    const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart');let server;
    const cli=command=>{const r=spawnSync(binary,['--config',config,command],{env,encoding:'utf8'});assert.equal(r.status,0,`CLI ${command} failed (private output omitted)`);};
    const start=async()=>{server=spawn(binary,['--config',config,'serve'],{env,stdio:'ignore'});for(let i=0;i<100;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Readiness failed');};
    const stop=async()=>{if(server&&server.exitCode===null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
    const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
    try {
      await db.query(`CREATE SCHEMA ${schema}`);await db.query(`SET search_path TO ${schema}`);cli('migrate');assert.deepEqual((await db.query("SELECT code,active,iban FROM payment_methods ORDER BY code")).rows,[{code:'bank_transfer',active:false,iban:''},{code:'cash_on_delivery',active:false,iban:''},{code:'online_checkout',active:false,iban:''}]);await start();
      const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
      await context.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);await page.goto(origin+'/setup');const password=randomBytes(20).toString('hex');
      await page.getByLabel('Nome del negozio',{exact:true}).fill('Payment methods');await page.getByLabel('Nome visualizzato',{exact:true}).fill('Test owner');await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);if(name==='desktop-demo')await page.locator('#install-demo').check();await page.getByRole('button',{name:'Crea negozio',exact:true}).click();await page.waitForURL('**/admin/login');
      await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);await page.getByRole('button',{name:'Accedi',exact:true}).click();await page.waitForURL('**/admin');
      assert.equal(Number((await db.query('SELECT count(*) FROM catalog_products')).rows[0].count)>0,name==='desktop-demo');
      const classId=await chargeTaxes.fixture(db);
      await orderStatuses({page,db,origin,output,name});
      const codId=(await db.query("SELECT id FROM payment_methods WHERE code='cash_on_delivery'")).rows[0].id;
      const codPath=origin+`/admin/settings/payments/${codId}`;
      const save=async(status=303)=>{const response=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().includes('/settings/payments/'));await page.locator('main button[type=submit]').click();assert.equal((await response).status(),status);if(status===303)await page.waitForURL('**/admin/settings/payments');};
      for(const lang of ['it','en']) {
        await db.query('UPDATE administrators SET locale=$1',[lang]);await page.goto(origin+'/admin/settings/payments');
        assert.ok((await page.locator('h1').innerText()).includes(lang==='it'?'Metodi di pagamento':'Payment methods'));
        assert.equal(await page.locator('.sidebar-submenu [aria-current=page]').count(),1);
        assert.equal(await page.locator('main a[href$="/new"]').count(),0);
        await page.screenshot({path:path.join(output,`${name}-${lang}-list.png`),fullPage:true});
        await page.goto(origin+'/admin/settings/payments/1');await orderStatuses.checkReturnLink(page,'/admin/settings/payments');await page.locator('#payment-active').check();await page.locator('#payment-iban').fill('INVALID');await save(422);
        assert.equal(await page.locator('#payment-iban').getAttribute('aria-invalid'),'true');assert.equal(await page.locator('#payment-iban').inputValue(),'INVALID');
        await page.locator('#payment-beneficiary').fill('Synthetic beneficiary');
        for (const iban of ['ZZ8112345678901','IT5812345678901']) {
          await page.locator('#payment-iban').fill(iban);await save(422);
          assert.equal(await page.locator('#payment-iban').getAttribute('aria-invalid'),'true');
          assert.equal(await page.locator('#payment-iban').inputValue(),iban);
        }
        await page.locator('#payment-order-status').selectOption('2');await page.locator('#payment-iban').fill('IT60 X054 2811 1010 0000 0123 456');await page.locator('#payment-fee-kind').selectOption('percent');await page.locator('#payment-fee_value').fill('2,50');await page.locator('#payment-minimum').fill('10');await chargeTaxes.missingClass({page,save,prefix:'payment',lang,classId});await save();
        const row=(await db.query('SELECT active,minimum_cents,fee_value,iban,initial_order_status_id FROM payment_methods WHERE id=1')).rows[0];assert.equal(row.initial_order_status_id,'2');assert.equal(row.active,true);assert.equal(row.minimum_cents,'1000');assert.equal(row.fee_value,'250');assert.equal(row.iban,'IT60X0542811101000000123456');
        await page.goto(origin+'/admin/settings/payments/1');
        assert.equal(await page.locator('#payment-minimum').inputValue(),'10.00');
        assert.equal(await page.locator('[name=kind]').count(),0);
        await page.locator('#payment-name_it').focus();await page.keyboard.press('Tab');assert.equal(await page.locator('#payment-name_en').evaluate(e=>e===document.activeElement),true);
        assert.ok(await page.locator('#payment-name_en').evaluate(e=>getComputedStyle(e).outlineStyle!=='none'));
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
        assert.ok(await page.locator('#payment-active').evaluate(e=>{const a=e.getBoundingClientRect(),b=e.closest('label').getBoundingClientRect();return Math.abs(a.top+a.height/2-b.top-b.height/2)<4;}),'Active checkbox and label align on one line');
        await page.screenshot({path:path.join(output,`${name}-${lang}-editor.png`),fullPage:true});
        await chargeTaxes.mode({page,db,save,prefix:'payment',lang});
        assert.equal((await page.goto(origin+'/admin/settings/payments/new')).status(),400);
        await page.goto(origin+'/admin/settings/payments/1');await page.locator('#payment-instructions_it').fill('<script>alert(1)</script>');await page.locator('#payment-fee-kind').selectOption('fixed');await page.locator('#payment-fee_value').fill('3.00');await page.locator('#payment-minimum').fill('');await save();
        await page.goto(origin+'/admin/settings/payments/1');assert.equal(await page.locator('#payment-minimum').inputValue(),'0.00');assert.equal(await page.locator('#payment-instructions_it').inputValue(),'<script>alert(1)</script>');
        assert.equal((await db.query('SELECT count(*) FROM payment_methods')).rows[0].count,'3');
        // An old open form cannot overwrite another administrator's save.
        await db.query('UPDATE payment_configuration SET revision=revision+1');await page.locator('#payment-minimum').fill('12.00');await save(409);assert.equal(await page.locator('#payment-minimum').inputValue(),'12.00');
      }
      const bankBefore=(await db.query('SELECT to_jsonb(m) AS value FROM payment_methods m WHERE id=1')).rows[0].value;
      for(const lang of ['it','en']) {
        await db.query('UPDATE administrators SET locale=$1',[lang]);await page.goto(origin+'/admin/settings/payments');
        await page.screenshot({path:path.join(output,`${name}-${lang}-cod-list.png`),fullPage:true});
        await page.goto(codPath);await orderStatuses.checkReturnLink(page,'/admin/settings/payments');
        assert.equal(await page.locator('h2').innerText(),lang==='it'?'Contrassegno':'Cash on delivery');
        for(const field of ['iban','bic','beneficiary','kind'])assert.equal(await page.locator(`[name=${field}]`).count(),0);
        await page.locator('#payment-order-status').selectOption('3');await page.locator('#payment-active').check();await page.locator('#payment-fee-kind').selectOption('percent');await page.locator('#payment-fee_value').fill('100.01');await page.locator('#payment-minimum').fill('10');await save(422);
        assert.equal(await page.locator('#payment-fee_value').getAttribute('aria-invalid'),'true');assert.equal(await page.locator('#payment-fee_value').inputValue(),'100.01');
        await page.locator('#payment-fee_value').fill('2,50');await chargeTaxes.missingClass({page,save,prefix:'payment',lang,classId});await save();
        const row=(await db.query('SELECT active,fee_kind,fee_value,minimum_cents,iban,initial_order_status_id FROM payment_methods WHERE id=$1',[codId])).rows[0];assert.deepEqual(row,{active:true,fee_kind:'percent',fee_value:'250',minimum_cents:'1000',iban:'',initial_order_status_id:'3'});
        await page.goto(codPath);await page.locator('#payment-name_it').focus();await page.keyboard.press('Tab');assert.equal(await page.locator('#payment-name_en').evaluate(e=>e===document.activeElement),true);
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
        await page.screenshot({path:path.join(output,`${name}-${lang}-cod-editor.png`),fullPage:true});
        await page.locator('#payment-fee-kind').selectOption('fixed');await page.locator('#payment-fee_value').fill('3.50');await page.locator('#payment-minimum').fill('');await save();
        await page.goto(codPath);assert.equal(await page.locator('#payment-minimum').inputValue(),'0.00');
        await db.query('UPDATE payment_configuration SET revision=revision+1');await page.locator('#payment-minimum').fill('12.00');await save(409);assert.equal(await page.locator('#payment-minimum').inputValue(),'12.00');
      }
      assert.deepEqual((await db.query('SELECT to_jsonb(m) AS value FROM payment_methods m WHERE id=1')).rows[0].value,bankBefore);
      await stop();cli('doctor');await start();await page.goto(origin+'/admin/settings/payments/1');assert.equal(await page.locator('#payment-minimum').inputValue(),'0.00');assert.equal(await page.locator('#payment-active').isChecked(),true);
      await page.goto(codPath);assert.equal(await page.locator('#payment-order-status').inputValue(),'3');assert.equal(await page.locator('#payment-active').isChecked(),true);assert.equal(await page.locator('#payment-fee_value').inputValue(),'3.50');assert.equal(await page.locator('#payment-minimum').inputValue(),'0.00');
      assert.deepEqual(errors,[]);results.push({name,languages:['it','en'],javascript:js,passed:true});console.log(name,'passed');
    } finally {await stop();await context.close();await db.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);fs.rmSync(folder,{recursive:true,force:true});}
  }} finally {await browser.close();await db.end();}
  fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
})().catch(e=>{console.error(e);process.exitCode=1;});
