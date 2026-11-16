// Real first installation, in isolated schemas and media roots; no application database writes.
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
  const owner = new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL}); await owner.connect();
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? {channel:process.env.TALOCART_BROWSER_CHANNEL} : {});
  const output=path.resolve('.local/verification/installation-languages'); fs.mkdirSync(output,{recursive:true});
  try {
    for (const [name,width,js,primary,active,demo] of [
      ['english-only-desktop',1440,true,'en',['en'],false],
      ['english-only-mobile-no-js',390,false,'en',['en'],true],
      ['english-primary-mobile',390,true,'en',['it','en'],true],
      ['italian-only-no-js',1440,false,'it',['it'],false],
      ['italian-primary-desktop',1440,true,'it',['it','en'],true],
    ]) {
      const schema=`talocart_install_languages_${process.pid}_${Date.now()}`;
      const folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-install-languages-'));
      const origin='http://127.0.0.1:3104';
      const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options',`-csearch_path=${schema}`);
      const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3104"\n',{mode:0o600});
      const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3104',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn',TALOCART_STORE_NAME:'Koala'};
      const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart');
      const cli=(...args)=>{const result=spawnSync(binary,['--config',config,...args],{env,encoding:'utf8'});assert.equal(result.status,0,`CLI ${args[0]} failed (private output omitted)`);};
      const query=async(sql,parameters)=>{await owner.query(`SET search_path TO ${schema}`);return owner.query(sql,parameters);};
      let server;
      const context=await browser.newContext({viewport:{width,height:950},javaScriptEnabled:js});
      try {
        await owner.query(`CREATE SCHEMA ${schema}`);cli('migrate');cli('migrate');
        const start=async()=>{server=spawn(binary,['--config',config,'serve'],{env,stdio:'ignore'});for(let i=0;i<100;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw new Error('Readiness failed');};
        const stop=async()=>{if(server && server.exitCode===null){server.kill('SIGTERM');await once(server,'exit');}server=undefined;};
        await start();
        const page=await context.newPage();const errors=[];page.on('pageerror',error=>errors.push(error.message));await page.goto(origin+'/setup');
        const platformBrand=async()=>{assert.match(await page.title(),/Talocart$/);assert.equal(await page.locator('.wordmark').innerText(),'Talocart');assert.ok(!(await page.locator('body').innerText()).includes('Koala'));};
        await platformBrand();
        assert.equal(await page.locator('html').getAttribute('lang'),'en');
        assert.equal(await page.locator('#access-language').inputValue(),'en');
        await page.locator('#access-language').focus();
        assert.equal(await page.locator('#access-language').evaluate(el=>el===document.activeElement),true);
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
        await page.screenshot({path:path.join(output,`setup-english-default-${name}.png`),fullPage:true});
        await page.locator('#access-language').selectOption('it');
        await page.locator('form[action="/admin/language"] button').click();
        await page.waitForLoadState('networkidle');
        assert.equal(await page.locator('html').getAttribute('lang'),'it');
        await platformBrand();
        await require('./ui-contract.cjs')(page);assert.equal(await page.locator('input[name="token"]').count(),0);
        const choiceLayout=await page.locator('.setup-languages input[type="checkbox"]').evaluateAll(inputs=>inputs.map(input=>{
          const label=input.closest('label'); const text=[...label.childNodes].find(node=>node.nodeType===Node.TEXT_NODE && node.textContent.trim());
          const range=document.createRange();range.selectNodeContents(text);
          const control=input.getBoundingClientRect(),caption=range.getBoundingClientRect();
          return {height:label.getBoundingClientRect().height,delta:Math.abs((control.top+control.bottom)/2-(caption.top+caption.bottom)/2),gap:caption.left-control.right};
        }));
        assert.ok(choiceLayout.every(row=>row.height>=44 && row.delta<3 && row.gap>=6 && row.gap<20),`Checkbox and text share an accessible row: ${JSON.stringify(choiceLayout)}`);
        assert.deepEqual(await page.locator('#setup-primary-locale option').evaluateAll(options=>options.map(option=>option.value)),['it','en']);
        assert.equal(await page.locator('#setup-primary-locale').inputValue(),'it');
        const password=randomBytes(20).toString('hex');
        const fill=async (storeName='Language test')=>{await page.getByLabel('Nome del negozio',{exact:true}).fill(storeName);await page.getByLabel('Email amministratore').fill('install@example.test');await page.getByLabel('Password',{exact:true}).fill(password);};
        const choices=async(primary,active)=>{await page.locator('#setup-primary-locale').selectOption(primary);for(const code of ['it','en']) await page.locator(`input[name="locale"][value="${code}"]`).setChecked(active.includes(code));};
        // Native validation messages, preserved choices, no partial installation.
        await fill();await choices('en',['it']);await page.getByLabel('Installa dati demo',{exact:true}).setChecked(demo);
        let rejected=page.waitForResponse(r=>new URL(r.url()).pathname==='/setup'&&r.request().method()==='POST');await page.getByRole('button',{name:'Crea negozio'}).click();assert.equal((await rejected).status(),422);
        assert.equal(await page.locator('#setup-primary-locale').inputValue(),'en');assert.equal(await page.locator('#setup-primary-locale').getAttribute('aria-invalid'),'true');assert.equal(await page.locator('#setup-primary-locale').getAttribute('aria-describedby'),'setup-languages-help setup-primary-error');assert.equal(await page.locator('input[name="locale"][value="it"]').isChecked(),true);
        await fill('   ');await choices(primary,active);rejected=page.waitForResponse(r=>new URL(r.url()).pathname==='/setup'&&r.request().method()==='POST');await page.getByRole('button',{name:'Crea negozio'}).click();assert.equal((await rejected).status(),422);assert.equal(await page.locator('#setup-primary-locale').inputValue(),primary);assert.equal(await page.getByLabel('Installa dati demo',{exact:true}).isChecked(),demo);
        await platformBrand();
        assert.equal((await query('SELECT installed FROM identity_state')).rows[0].installed,false);assert.equal(Number((await query('SELECT count(*) FROM storefront_language_prefixes')).rows[0].count),0);
        await fill();
        await page.locator('#setup-primary-locale').focus();assert.equal(await page.locator('#setup-primary-locale').evaluate(el=>el===document.activeElement),true);
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
        await page.locator('.setup-languages').scrollIntoViewIfNeeded();await page.screenshot({path:path.join(output,`setup-${name}.png`),fullPage:width>500});
        await page.getByRole('button',{name:'Crea negozio'}).focus();await page.keyboard.press('Enter');await page.waitForURL('**/admin/login');
        assert.equal((await page.locator('html').getAttribute('lang')),primary);
        assert.equal((await query('SELECT primary_locale FROM storefront_settings')).rows[0].primary_locale,primary);assert.deepEqual((await query('SELECT code FROM storefront_languages ORDER BY code')).rows.map(row=>row.code),[...active].sort());assert.equal((await query('SELECT locale FROM administrators')).rows[0].locale,primary);assert.equal(Number((await query('SELECT count(*) FROM catalog_products')).rows[0].count),demo?30:0);
        const fiscalDefaults=async()=>{
          const rows=(await query("SELECT basis_points,explicit_rule FROM resolve_tax('IT') WHERE tax_name LIKE '% / %' ORDER BY basis_points")).rows;
          assert.deepEqual(rows.map(row=>[row.basis_points,row.explicit_rule]),[[0,true],[400,true],[500,true],[1000,true],[2200,true]]);
          assert.equal((await query("SELECT country_code FROM pricing_settings")).rows[0].country_code,'IT');
        };
        await fiscalDefaults();
        await page.locator('#email').fill('install@example.test');await page.locator('#password').fill(password);
        await page.locator('form[action="/admin/login"] button[type="submit"]').click();await page.waitForURL('**/admin');
        for(const section of ['rates','classes','zones']) {
          assert.equal((await page.goto(origin+'/admin/settings/taxes/'+section)).status(),200);
          assert.equal(await page.locator('html').getAttribute('lang'),primary);
          assert.equal(await page.locator('.catalog-row').count(),section==='zones'?1:(demo?8:5));
          assert.ok((await page.locator('main').innerText()).includes(section==='zones'?'Italia / Italy':'Esente IVA / VAT exempt 0%'));
          await require('./ui-contract.cjs')(page);
          assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
          await page.screenshot({path:path.join(output,`tax-${section}-${name}.png`),fullPage:true});
        }
        await stop();await start();await fiscalDefaults();await page.goto(origin+'/catalog');assert.equal(await page.locator('html').getAttribute('lang'),primary);
        assert.equal(await page.locator('.store-languages').count(),active.length>1?1:0);
        assert.equal(await page.locator('link[rel="canonical"]').getAttribute('href'),origin+'/catalog');
        assert.equal(await page.locator(`a[href^="/${primary}/"]`).count(),0,'Links are generated directly without primary prefix');
        const alias=await context.request.get(origin+`/${primary}/catalog?filter=a%26b`,{maxRedirects:0});assert.equal(alias.status(),404);assert.equal(alias.headers().location,undefined);
        if(demo) {await page.locator('.store-product a').first().click();assert.match(new URL(page.url()).pathname,/^\/products\//);}
        for(const code of active.filter(code=>code!==primary)){const response=await page.goto(origin+`/${code}/catalog`);assert.equal(response.status(),200);assert.equal(await page.locator('html').getAttribute('lang'),code);}
        await page.goto(origin+'/catalog');await page.screenshot({path:path.join(output,`catalog-${name}.png`),fullPage:width>500});assert.deepEqual(errors,[]);
        console.log(`Installation languages ${name}: validation, choices, keyboard, direct URLs, restart and demo ${demo}: OK`);
        await stop();
      } finally {
        await context.close();if(server&&server.exitCode===null){server.kill('SIGTERM');await once(server,'exit');}
        await owner.query('SET search_path TO public');await owner.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);fs.rmSync(folder,{recursive:true,force:true});
      }
    }
  } finally {await browser.close();await owner.end();}
})().catch(error=>{console.error(error);process.exitCode=1;});
