// Product page size and product/order table typography, using synthetic data only.
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {randomBytes}=require('node:crypto'),{spawn,spawnSync}=require('node:child_process'),{once}=require('node:events');
const {Client}=require('pg'),{chromium}=require('playwright');
(async()=>{
 assert.ok(process.env.TALOCART_TEST_DATABASE_URL,'Dedicated test database required');
 const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 const schema='talocart_product_controls_'+process.pid+'_'+Date.now(),folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-product-controls-'));
 const origin='http://127.0.0.1:3141',output=path.resolve('.local/verification/product-list-controls');fs.mkdirSync(output,{recursive:true});
 const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options','-csearch_path='+schema);
 const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3141"\n',{mode:0o600});
 const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/debug/talocart');
 const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3141',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
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
  await db.query("INSERT INTO catalog_products(name,model,slug,price_cents) SELECT 'Large page item ' || lpad(i::text,3,'0'),'LARGE-' || lpad(i::text,3,'0'),'large-page-' || i,100 FROM generate_series(1,515) i");
  await db.query("UPDATE catalog_products SET description='Synthetic <b>description</b> & details.\nSecond line.' WHERE model<>'LARGE-043'");
  const imageKey='c'.repeat(64)+'.png',seedImage=fs.readFileSync(path.resolve('seeders/catalog-demo/assets/shirt-sage.png'));
  fs.mkdirSync(path.join(folder,'media','objects'),{recursive:true});fs.writeFileSync(path.join(folder,'media','objects',imageKey),seedImage);
  await db.query("INSERT INTO media_objects(key,content_type,byte_size,width,height,public) VALUES($1,'image/png',$2,512,512,true)",[imageKey,seedImage.length]);
  await db.query('UPDATE catalog_products SET image_key=$1',[imageKey]);
  await db.query("INSERT INTO operation_events(created_at,area,resource,resource_key,action) SELECT '2099-01-15 12:30:00+00'::timestamptz,'catalog','catalog_products',id::text,'UPDATE' FROM catalog_products");
  for(const theme of ['default','essential'])for(const locale of ['it','en'])for(const[profile,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]){
   await db.query('UPDATE storefront_settings SET theme_id=$1',[theme]);
   await db.query('UPDATE administrators SET locale=$1',[locale]);
   const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});await context.addCookies(cookies);const page=await context.newPage(),errors=[];page.on('pageerror',error=>errors.push(error.message));
   try{
    const label=locale==='it'?'Prodotti per pagina':'Products per page';
    const apply=locale==='it'?'Applica':'Apply';
    await page.goto(origin+'/admin/catalog/products?q=Large&type=simple&sort=model&dir=asc&page=2');
    await require('./browser-product-table-options.cjs')({page,context,locale,js,width,origin,output,profile,theme});
    await page.goto(origin+'/admin/catalog/products?q=Large&type=simple&sort=model&dir=asc&page=2');
    assert.deepEqual(await page.locator('#product-page-size option').evaluateAll(items=>items.map(item=>item.value)),['10','20','50','100','500']);
    await page.getByLabel(label,{exact:true}).focus();
    assert.equal(await page.getByLabel(label,{exact:true}).evaluate(el=>el===document.activeElement),true);
    await page.getByLabel(label,{exact:true}).selectOption('500');
    if(!js)await page.getByRole('button',{name:apply,exact:true}).click();
    await page.waitForURL(url=>url.searchParams.get('per_page')==='500'&&!url.searchParams.has('page'));
    for(const[key,value]of Object.entries({q:'Large',type:'simple',sort:'model',dir:'asc'}))assert.equal(new URL(page.url()).searchParams.get(key),value);
    assert.equal(await page.locator('.ui-table__link').count(),500);
    const button=page.locator('.product-page-size button[type=submit]');await button.waitFor({state:js?'hidden':'visible'});
    await page.locator('a[rel=next]').click();
    assert.equal(await page.locator('.ui-table__link').count(),15);
    assert.equal(await page.getByLabel(label,{exact:true}).inputValue(),'500');
    assert.ok((await page.locator('.pagination-summary').innerText()).includes('501–515'));
    const preference=(await context.cookies()).find(cookie=>cookie.name==='talocart_admin_product_page_size');
    assert.ok(preference&&preference.value==='500'&&preference.expires>Date.now()/1000+179*86400,'Persistent admin page size');
    await page.locator('tbody [data-product-column=actions] a').first().click();
    await page.locator('#product-seo_title').fill(`Preference ${theme} ${locale} ${profile}`);
    await Promise.all([page.waitForURL(url=>url.searchParams.get('saved')==='1'),page.getByRole('button',{name:locale==='it'?'Salva prodotto':'Save product',exact:true}).click()]);
    await page.locator('a.back-link[href="/admin/catalog/products"]').click();
    assert.equal(new URL(page.url()).search,'','Return from editor has no page size parameter');
    assert.equal(await page.getByLabel(label,{exact:true}).inputValue(),'500','Saved article preserves page size');
    assert.equal(await page.locator('.ui-table__link').count(),500);
    assert.equal(await page.locator('.ui-page-header').count(),1,'Shared page header');
    assert.equal(await page.locator('.ui-page-header > :not(.ui-page-header__bar), .ui-page-header__bar > :not(h1, .ui-page-header__actions)').count(),0,'Title without eyebrow or subtitle');
    await page.reload();assert.equal(await page.getByLabel(label,{exact:true}).inputValue(),'500');
    const reopened=await browser.newContext({storageState:await context.storageState(),viewport:{width,height:1000},javaScriptEnabled:js});
    try{const tab=await reopened.newPage();await tab.goto(origin+'/admin/catalog/products');assert.equal(await tab.locator('#product-page-size').inputValue(),'500','Preference survives browser context reopening');}finally{await reopened.close();}
    const headerMetrics=[];
    for(const route of ['/admin/catalog/products?per_page=500&page=2','/admin/orders']){
     await page.goto(origin+route);
     assert.equal(await page.locator('html').getAttribute('lang'),locale);
     const metrics=await page.locator('.ui-table thead tr:first-child th').evaluateAll(items=>items.map(el=>({font:parseFloat(getComputedStyle(el).fontSize),root:parseFloat(getComputedStyle(document.documentElement).fontSize)})));
     assert.ok(metrics.length>1);for(const metric of metrics)assert.ok(Math.abs(metric.font-metric.root*.8)<.1,route+': larger headers');
     headerMetrics.push({route,metrics});
     const link=page.locator('.ui-table__sort').first();await link.focus();assert.equal(await link.evaluate(el=>el===document.activeElement),true);assert.notEqual(await link.evaluate(el=>getComputedStyle(el).outlineStyle),'none');
     assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'No page overflow');
     if(width>=1280)assert.ok(await page.locator('.ui-table-region').evaluate(el=>el.scrollWidth<=Math.max(el.clientWidth,el.querySelector('[data-product-column=status]')?1280:0)+1),'Table fits its readable width without page overflow');
     await page.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-'+(route.includes('/orders')?'orders':'products')+'.png')});
     if(route==='/admin/orders')assert.deepEqual(await page.locator('#order-page-size option').evaluateAll(items=>items.map(item=>item.value)),['10','20','50','100']);
    }
    fs.writeFileSync(path.join(output,theme+'-'+locale+'-'+profile+'-metrics.json'),JSON.stringify(headerMetrics,null,2));
    assert.deepEqual(errors,[]);
    results.push({theme,locale,profile,passed:true});console.log(theme,locale,profile,'passed');
   }finally{await context.close();}
  }
  await admin.goto(origin+'/admin/catalog/products');await admin.locator('[data-product-columns] summary').click();
  let saved=admin.waitForResponse(r=>r.url().endsWith('/products/columns')&&r.request().method()==='POST');
  await admin.locator('input[name=columns][value=id]').uncheck();assert.equal((await saved).status(),204);
  await admin.goto(origin+'/admin/catalog/products?per_page=50');
  await stop();await start();await admin.goto(origin+'/admin/catalog/products');
  assert.equal(await admin.locator('#product-page-size').inputValue(),'50','Page size survives server restart');
  assert.equal(await admin.locator('thead th[data-product-column=id]').isVisible(),false,'Preference survives server restart');
  await admin.locator('[data-product-columns] summary').click();saved=admin.waitForResponse(r=>r.url().endsWith('/products/columns')&&r.request().method()==='POST');
  await admin.locator('[data-columns-reset]').click();assert.equal((await saved).status(),204);
  await setup.close();fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
 }finally{await stop();await browser.close();await db.query('DROP SCHEMA IF EXISTS '+schema+' CASCADE');await db.end();fs.rmSync(folder,{recursive:true,force:true});}
})().catch(e=>{console.error(e.stack);process.exitCode=1;});
