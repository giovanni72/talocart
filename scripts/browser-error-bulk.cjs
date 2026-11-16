// Full-page bulk selection and contextual error documents, using synthetic data only.
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {randomBytes}=require('node:crypto'),{spawn,spawnSync}=require('node:child_process'),{once}=require('node:events');
const {Client}=require('pg'),{chromium}=require('playwright');
(async()=>{
 assert.ok(process.env.TALOCART_TEST_DATABASE_URL,'Dedicated test database required');
 const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 const schema='talocart_error_bulk_'+process.pid+'_'+Date.now(),folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-error-bulk-'));
 const origin='http://127.0.0.1:3142',output=path.resolve('.local/verification/error-bulk');fs.mkdirSync(output,{recursive:true});
 const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options','-csearch_path='+schema);
 const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3142"\n',{mode:0o600});
 const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart');
 const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3142',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
 let server;const results=[];
 const start=async()=>{const fd=fs.openSync(path.join(output,'server.log'),'a',0o600);server=spawn(binary,['--config',config,'serve'],{env,stdio:['ignore',fd,fd]});fs.closeSync(fd);for(let i=0;i<150;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Readiness');};
 const stop=async()=>{if(server&&server.exitCode===null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
 try{
  await db.query('CREATE SCHEMA '+schema);await db.query('SET search_path TO '+schema);
  assert.equal(spawnSync(binary,['--config',config,'migrate'],{env,stdio:'pipe'}).status,0,'Clean migration');await start();
  const setup=await browser.newContext(),admin=await setup.newPage(),password=randomBytes(20).toString('hex');
  await setup.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);await admin.goto(origin+'/setup');
  await admin.getByLabel('Nome del negozio',{exact:true}).fill('Product list controls');await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);
  await admin.getByRole('button',{name:'Crea negozio',exact:true}).click();await admin.waitForURL('**/admin/login');
  await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);await admin.getByRole('button',{name:'Accedi',exact:true}).click();await admin.waitForURL('**/admin');
  const cookies=(await setup.cookies()).filter(c=>c.name==='talocart_session');
  await db.query("INSERT INTO storefront_languages(code) VALUES('it'),('en') ON CONFLICT DO NOTHING");
  const primary=(await db.query('SELECT primary_locale FROM storefront_settings')).rows[0].primary_locale;
  await db.query("INSERT INTO catalog_products(name,model,slug,price_cents,status,tax_class_id) SELECT 'Bulk page '||lpad(i::text,3,'0'),'BULK-PAGE-'||lpad(i::text,3,'0'),'bulk-page-'||i,100,'published',(SELECT id FROM tax_classes ORDER BY id LIMIT 1) FROM generate_series(1,515)i");
  for(const theme of ['default','essential'])for(const locale of ['it','en'])for(const[profile,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]){
   const caseName=theme+'-'+locale+'-'+profile;
   if(process.env.TALOCART_ERROR_BULK_CASE && process.env.TALOCART_ERROR_BULK_CASE!==caseName)continue;
   await db.query('UPDATE storefront_settings SET theme_id=$1',[theme]);await db.query('UPDATE administrators SET locale=$1',[locale]);
   await db.query("UPDATE catalog_products SET status='published'");
   const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});await context.addCookies(cookies);const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
   try{
    const mode=process.env.TALOCART_BROWSER_REGRESSION||'all';
    if(mode!=='errors'){
     const target=origin+'/admin/catalog/products?model=BULK-PAGE&sort=model&dir=asc&per_page=500';await page.goto(target);
     assert.equal(await page.locator('.product-row-select').count(),500);
     if(js)await page.getByRole('checkbox',{name:locale==='it'?'Seleziona tutti gli articoli visibili':'Select all visible items',exact:true}).check();
     else {assert.equal(await page.locator('#product-select-all').isDisabled(),true);for(let i=0;i<3;i++)await page.locator('.product-row-select').nth(i).check();}
     const count=js?500:3;assert.equal(await page.locator('.product-row-select:checked').count(),count);
     const ids=await page.locator('.product-row-select:checked').evaluateAll(rows=>rows.map(r=>r.value));
     const preview=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().endsWith('/products/bulk'));
     await page.locator('#product-bulk-button').click();assert.equal((await preview).status(),200,'The entire 500-row selection must reach the bulk preview');
     assert.equal(await page.locator('input[name=ids]').count(),count);
     await page.screenshot({path:path.join(output,caseName+'-bulk.png'),fullPage:true});
     await page.locator('input[name=action][value=draft]').check();await page.locator('input[name=confirm]').check();
     await page.getByRole('button',{name:locale==='it'?'Applica azioni':'Apply actions',exact:true}).click();
     await page.waitForURL(url=>url.searchParams.get('bulk_status')==='draft');
     assert.equal(new URL(page.url()).searchParams.get('per_page'),'500');
     assert.equal(Number((await db.query("SELECT count(*) FROM catalog_products WHERE id=ANY($1) AND status='draft'",[ids])).rows[0].count),count);
     assert.equal(Number((await db.query("SELECT count(*) FROM catalog_products WHERE status='published'")).rows[0].count),515-count);
    }
    if(mode!=='bulk'){
     const prefix=locale===primary?'':'/'+locale;
     for(const route of ['/products/missing','/pages/missing','/missing','/catalog?per_page=invalid']){
      const response=await page.goto(origin+prefix+route);assert.equal(response.status(),route.includes('?')?422:404);
      assert.equal(await page.locator('html').getAttribute('lang'),locale);
      assert.equal(await page.locator('.site-header').count(),1,'Frontend error retains the store header');
      assert.equal(await page.locator('.site-footer').count(),1);
      assert.equal(await page.locator('body').getAttribute('data-theme'),theme);
      assert.equal(await page.locator('.admin-shell').count(),0);
      assert.equal(await page.locator('main .ui-button').first().getAttribute('href'),prefix+'/catalog');
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
      if(route==='/products/missing')await page.screenshot({path:path.join(output,caseName+'-frontend.png'),fullPage:true});
     }
     await page.goto(origin+'/admin/catalog/products?per_page=10');
     await page.locator('.product-row-select').first().check();
     await page.route('**/admin/catalog/products/bulk',route=>{const data=new URLSearchParams(route.request().postData());data.set('csrf','expired-synthetic-token');return route.continue({postData:data.toString()});});
     const forbidden=page.waitForResponse(r=>r.url().endsWith('/products/bulk')&&r.request().method()==='POST');
     await page.locator('#product-bulk-button').click();assert.equal((await forbidden).status(),403);
     assert.equal(await page.locator('.admin-shell').count(),1);
     assert.equal(await page.locator('h1').innerText(),locale==='it'?'Accesso non consentito':'Access not allowed');
     await page.screenshot({path:path.join(output,caseName+'-forbidden.png'),fullPage:true});
     await page.unroute('**/admin/catalog/products/bulk');
     for(const route of ['/admin/catalog/products/9223372036854775807','/admin/unknown','/admin/catalog/products?id=bad']){
      const response=await page.goto(origin+route);assert.equal(response.status(),route.includes('?')?422:404);
      assert.equal(await page.locator('.admin-shell').count(),1,'Backend error retains admin navigation');
      assert.equal(await page.locator('.admin-sidebar').count(),1);
      assert.equal(await page.locator('.site-footer').count(),0);
      assert.equal(await page.locator('html').getAttribute('lang'),locale);
      if(!route.includes('?'))await page.screenshot({path:path.join(output,caseName+'-backend.png'),fullPage:true});
     }
    }
    assert.deepEqual(errors,[]);results.push({theme,locale,profile,mode,bulk_count:js?500:3,passed:true});console.log(caseName+' passed');
   }catch(e){await page.screenshot({path:path.join(output,caseName+'-failure.png'),fullPage:true}).catch(()=>{});throw e;}finally{await context.close();}
  }
  await setup.close();fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
 }finally{await stop();await browser.close();await db.query('DROP SCHEMA IF EXISTS '+schema+' CASCADE');await db.end();fs.rmSync(folder,{recursive:true,force:true});}
})().catch(e=>{console.error(e.stack);process.exitCode=1;});
