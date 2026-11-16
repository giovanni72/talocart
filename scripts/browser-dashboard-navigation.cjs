// Dedicated schemas and synthetic accounts only. Shared navigation across both themes.
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {randomBytes}=require('node:crypto'),{spawn,spawnSync}=require('node:child_process'),{once}=require('node:events');
const {Client}=require('pg'),{chromium}=require('playwright');
(async()=>{
 assert.ok(process.env.TALOCART_TEST_DATABASE_URL,'Dedicated test database required');
 const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 const schema='talocart_dashboard_'+process.pid+'_'+Date.now(),folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-dashboard-'));
 const origin='http://127.0.0.1:3138',output=path.resolve('.local/verification/dashboard-navigation/browser');fs.mkdirSync(output,{recursive:true});
 const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options','-csearch_path='+schema);
 const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3138"\n',{mode:0o600});
 const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart');
 const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3138',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
 let server;const results=[];
 const start=async()=>{const fd=fs.openSync(path.join(output,'server.log'),'a',0o600);server=spawn(binary,['--config',config,'serve'],{env,stdio:['ignore',fd,fd]});fs.closeSync(fd);for(let i=0;i<150;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Readiness');};
 const stop=async()=>{if(server&&server.exitCode===null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
 try{
  await db.query('CREATE SCHEMA '+schema);await db.query('SET search_path TO '+schema);
  assert.equal(spawnSync(binary,['--config',config,'migrate'],{env,stdio:'pipe'}).status,0,'Clean migration');await start();
  const setup=await browser.newContext(),admin=await setup.newPage(),password=randomBytes(20).toString('hex');
  await setup.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);await admin.goto(origin+'/setup');
  await admin.getByLabel('Nome del negozio',{exact:true}).fill('Dashboard navigation');await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);
  await admin.getByRole('button',{name:'Crea negozio',exact:true}).click();await admin.waitForURL('**/admin/login');
  await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);await admin.getByRole('button',{name:'Accedi',exact:true}).click();await admin.waitForURL('**/admin');
  await db.query(`INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents,created_at)
    SELECT 'CHART-'||n,n,0,repeat('a',64),'status_id = '||s.id,'EUR',n*100,0,0,n*100,0,n*100,statement_timestamp()-interval '2 hours'
    FROM generate_series(1,2) n CROSS JOIN LATERAL (SELECT id FROM order_statuses ORDER BY id OFFSET n-1 LIMIT 1) s`);
  const statusIds=(await db.query('SELECT id FROM order_statuses ORDER BY id LIMIT 2')).rows.map(r=>String(r.id));
  const cookies=(await setup.cookies()).filter(c=>c.name==='talocart_session');
  for(const theme of ['default','essential'])for(const locale of ['it','en'])for(const[profile,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]){
   await db.query('UPDATE storefront_settings SET theme_id=$1',[theme]);
   await db.query('UPDATE administrators SET locale=$1',[locale]);
   const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});await context.addCookies(cookies);const page=await context.newPage(),errors=[];page.on('pageerror',error=>errors.push(error.message));
   const applySales=async(reset=false)=>{
    await page.waitForLoadState('networkidle');
    const groups=js?await page.locator('.dashboard-groups').elementHandle():null,requests=[];
    const record=request=>requests.push({type:request.resourceType(),path:new URL(request.url()).pathname});page.on('request',record);
    if(js)await page.evaluate(()=>{window.salesSettled=false;document.addEventListener('htmx:afterSettle',()=>{window.salesSettled=true;},{once:true});});
    const control=page.locator(reset?'#sales-reset':'#sales-apply');await control.focus();
    if(js){await control.press('Enter');await page.waitForFunction(()=>window.salesSettled);}
    else await Promise.all([page.waitForNavigation({waitUntil:'load'}),control.press('Enter')]);
    await page.waitForLoadState('networkidle');page.off('request',record);
    if(js){
     assert.equal(await groups.evaluate(el=>el.isConnected),true,'Only the sales section is replaced with JavaScript');await groups.dispose();
     // Chrome may refresh its no-store tab icon after pushState; it is independent
     // of the fragment. No document, stylesheet or script is requested again.
     assert.deepEqual(requests.filter(r=>r.path!=='/branding/favicon'),[{type:'xhr',path:'/admin'}],'Filter update sends one application request');
     assert.ok(requests.filter(r=>r.path==='/branding/favicon').length<=1);
     assert.equal(await page.locator(reset?'#sales-reset':'#sales-apply').evaluate(el=>el===document.activeElement),true,'Keyboard focus survives the swap');
    }else assert.equal(requests.filter(r=>r.type==='document').length,1,'Native GET loads a complete document');
   };
   try{
    await require('./browser-admin-width.cjs')({page,origin,output,theme,locale,profile});
    await page.goto(origin+'/admin');
    assert.equal(await page.locator('#sales-title').innerText(),locale==='it'?'Andamento delle vendite':'Sales trend');
    assert.equal(await page.locator('[data-sales-total]').getAttribute('data-sales-total'),locale==='it'?'3,00':'3.00');
    await page.selectOption('#sales-metric','orders');await page.selectOption('#sales-period','7d');
    await page.locator('.sales-status-filter summary').focus();await page.locator('.sales-status-filter summary').press('Enter');
    for(const checkbox of await page.locator('.sales-status-list input').all())await checkbox.uncheck();
    await page.locator('input[name="status"][value="'+statusIds[0]+'"]').check();
    await applySales();
    await page.waitForURL(/metric=orders/);
    assert.equal(await page.locator('[data-sales-total]').getAttribute('data-sales-total'),'1');
    assert.equal(await page.locator('#sales-period').inputValue(),'7d');
    assert.equal(await page.locator('.sales-status-list input:checked').count(),1);
    await page.locator('.sales-data summary').click();assert.equal(await page.locator('.sales-data tbody tr').count(),7);
    await page.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-sales.png'),fullPage:true});
    await page.locator('.sales-status-filter summary').click();for(const checkbox of await page.locator('.sales-status-list input').all())await checkbox.uncheck();
    await applySales();
    await page.waitForURL(url=>!url.searchParams.has('status'));
    assert.equal(await page.locator('[data-sales-total]').getAttribute('data-sales-total'),'0');
    assert.equal(await page.locator('.dashboard-sales .ui-empty').count(),1);
    for(const period of ['90d','12m']){
      await page.selectOption('#sales-period',period);
      await applySales();
      await page.waitForURL(url=>url.searchParams.get('period')===period);
      assert.equal(await page.locator('.sales-data tbody tr').count(),period==='12m'?12:90);
    }
    await applySales(true);
    assert.equal(await page.locator('#sales-period').inputValue(),'30d');
    assert.equal(await page.locator('#sales-metric').inputValue(),'amount');
    assert.equal(await page.locator('.sales-status-list input:checked').count(),await page.locator('.sales-status-list input').count());
    await page.goBack();
    await page.waitForFunction(()=>document.querySelector('#sales-period')?.value==='12m');
    assert.equal(await page.locator('#sales-period').inputValue(),'12m','Back restores the previous filter document');
    assert.equal(await page.locator('.admin-sidebar').count(),1);
    await page.reload();
    assert.equal(await page.locator('#sales-period').inputValue(),'12m','Reload retains the URL filters');
    if(js&&profile==='desktop'){
      let intercepted=false,release,finished;
      const entered=page.waitForRequest(r=>new URL(r.url()).searchParams.get('period')==='90d'),held=new Promise(resolve=>{release=resolve;}),handled=new Promise(resolve=>{finished=resolve;});
      const pattern=origin+'/admin?**';
      const intercept=async route=>{
        if(new URL(route.request().url()).searchParams.get('period')!=='90d')return route.continue();
        intercepted=true;await held;
        try{await route.abort();}catch(error){if(!route.request().failure())throw error;}finally{finished();}
      };
      await page.route(pattern,intercept);
      try{
        await page.selectOption('#sales-period','90d');await page.locator('#sales-apply').click();await entered;
        const cancelled=page.waitForEvent('requestfailed',{predicate:r=>new URL(r.url()).searchParams.get('period')==='90d'});
        await page.selectOption('#sales-period','12m');
        await page.evaluate(()=>{window.salesSettled=false;document.addEventListener('htmx:afterSettle',()=>{window.salesSettled=true;},{once:true});});
        await page.locator('#sales-apply').click();await cancelled;await page.waitForFunction(()=>window.salesSettled);
        assert.equal(await page.locator('#sales-period').inputValue(),'12m','A newer filter cancels the held request');
        assert.equal(await page.locator('.sales-data tbody tr').count(),12);
      }finally{release();if(intercepted)await handled;await page.unroute(pattern,intercept);}
    }
    const invalid=await page.goto(origin+'/admin?period=invalid');assert.equal(invalid.status(),422);
    await page.goto(origin+'/admin');
    assert.equal(await page.locator('main #status-result, main #check-status').count(),0,'Dashboard has no application status');
    assert.equal(await page.locator('main a[href="/admin/components"]').count(),0,'Components moved out of dashboard');
    assert.equal(await page.locator('main a[href="/admin/operations"]').count(),0,'Operations stays only in sidebar');
    assert.equal(await page.locator('.admin-sidebar nav a[href="/admin/operations"]').count(),1);
    const design=page.locator('.admin-sidebar details').filter({has:page.locator('summary', {hasText:'Design'})});
    assert.equal(await design.count(),1);
    assert.equal(await design.getAttribute('open'),null);
    assert.deepEqual(await design.locator('a').evaluateAll(links=>links.map(a=>a.getAttribute('href'))),['/admin/pages','/admin/banners']);
    await design.locator('summary').focus();await design.locator('summary').press('Enter');
    for(const route of ['/admin/pages','/admin/banners']){
      await design.locator('a[href="'+route+'"]').click();await page.waitForURL(origin+route);
      assert.notEqual(await design.getAttribute('open'),null);
      assert.equal(await design.locator('a[aria-current="page"]').getAttribute('href'),route);
    }
    await page.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-design.png'),fullPage:true});
    await page.goto(origin+'/admin');
    const maintenance=page.locator('.admin-sidebar details').filter({has:page.locator('summary', {hasText:locale==='it'?'Manutenzione':'Maintenance'})});
    assert.equal(await maintenance.count(),1);
    assert.equal(await maintenance.getAttribute('open'),null);
    assert.equal(await maintenance.evaluate(el=>el.previousElementSibling.querySelector('summary').textContent.trim()),locale==='it'?'Utenti':'Users');
    assert.deepEqual(await maintenance.locator('a').evaluateAll(links=>links.map(a=>a.getAttribute('href'))),['/admin/operations','/admin/updates','/admin/error-logs']);
    await maintenance.locator('summary').focus();await maintenance.locator('summary').press('Enter');
    assert.notEqual(await maintenance.getAttribute('open'),null);
    await page.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-maintenance.png'),fullPage:true});
    for(const route of ['/admin/operations','/admin/updates','/admin/error-logs']){
      await maintenance.locator('a[href="'+route+'"]').click();await page.waitForURL(origin+route);
      assert.notEqual(await maintenance.getAttribute('open'),null);
      assert.equal(await maintenance.locator('a[aria-current="page"]').getAttribute('href'),route);
    }
    await maintenance.locator('summary').focus();await maintenance.locator('summary').press('Enter');
    assert.equal(await maintenance.getAttribute('open'),null);
    const links=page.locator('.admin-sidebar nav a');
    assert.equal(await links.last().getAttribute('href'),'/admin/components');
    const link=links.last();assert.equal((await link.innerText()).trim(),locale==='it'?'Componenti dell’interfaccia':'Interface components');
    await link.focus();await link.press('Enter');await page.waitForURL('**/admin/components');
    assert.equal(await page.locator('.admin-sidebar nav a[aria-current="page"]').count(),1);
    assert.equal(await page.locator('.admin-sidebar nav a[aria-current="page"]').getAttribute('href'),'/admin/components');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    await page.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-components.png'),fullPage:true});
    await page.goto(origin+'/admin');
    await page.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-dashboard.png'),fullPage:true});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    assert.deepEqual(errors,[]);
    results.push({theme,locale,profile,passed:true});console.log(theme,locale,profile,'passed');
   }finally{await context.close();}
  }
  await setup.close();fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
 }finally{await stop();await browser.close();await db.query('DROP SCHEMA IF EXISTS '+schema+' CASCADE');await db.end();fs.rmSync(folder,{recursive:true,force:true});}
})().catch(e=>{console.error(e.stack);process.exitCode=1;});
