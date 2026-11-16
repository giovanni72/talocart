// Dedicated schemas and synthetic accounts only. Shared navigation across both themes.
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {randomBytes}=require('node:crypto'),{spawn,spawnSync}=require('node:child_process'),{once}=require('node:events');
const {Client}=require('pg'),{chromium}=require('playwright');
(async()=>{
 assert.ok(process.env.TALOCART_TEST_DATABASE_URL,'Dedicated test database required');
 const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 const schema='talocart_updates_'+process.pid+'_'+Date.now(),folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-updates-'));
 const origin='http://127.0.0.1:3156',output=path.resolve('.local/verification/updates/browser');fs.mkdirSync(output,{recursive:true});
 const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options','-csearch_path='+schema);
 const config=path.join(folder,'talocart.toml');const updateRoot=path.join(folder,'updates');fs.mkdirSync(path.join(updateRoot,'requests'),{recursive:true,mode:0o700});
 fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3156"\n[updates]\nroot='+JSON.stringify(updateRoot)+'\n',{mode:0o600});
 const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart');
 const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3156',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
 let server;const results=[];
 const start=async()=>{const fd=fs.openSync(path.join(output,'server.log'),'a',0o600);server=spawn(binary,['--config',config,'serve'],{env,stdio:['ignore',fd,fd]});fs.closeSync(fd);for(let i=0;i<150;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Readiness');};
 const stop=async()=>{if(server&&server.exitCode===null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
 try{
  await db.query('CREATE SCHEMA '+schema);await db.query('SET search_path TO '+schema);
  assert.equal(spawnSync(binary,['--config',config,'migrate'],{env,stdio:'pipe'}).status,0,'Clean migration');await start();
  const setup=await browser.newContext(),admin=await setup.newPage(),password=randomBytes(20).toString('hex');
  await setup.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);await admin.goto(origin+'/setup');
  await admin.getByLabel('Nome del negozio',{exact:true}).fill('Application updates');await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);
  await admin.getByRole('button',{name:'Crea negozio',exact:true}).click();await admin.waitForURL('**/admin/login');
  await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);await admin.getByRole('button',{name:'Accedi',exact:true}).click();await admin.waitForURL('**/admin');
  const cookies=(await setup.cookies()).filter(c=>c.name==='talocart_session');
  const candidate={version:'999.0.0',commit:'a'.repeat(40),digest:'b'.repeat(64)};
  const plan={release:'v999.0.0',sequence:21,base_from:'11.11.0',base_to:'999.0.0',base_action:'update',blocked:false,
   modules:[{id:'koala-import',from:'1.0.0',to:'2.0.0',enabled:false,action:'update',blocker:null},
    {id:'vendor-inventory',from:'3.0.0',to:'3.0.0',enabled:true,action:'keep',blocker:null}]};
  const writeStatus=(phase,extra={})=>fs.writeFileSync(path.join(updateRoot,'status.json'),JSON.stringify({format:2,phase,checked_at:'2026-11-01T10:00:00+00:00',candidate:['available','failed','blocked'].includes(phase)?candidate:null,plan,recovery_required:false,...extra}));
  for(const theme of ['default','essential'])for(const locale of ['it','en'])for(const[profile,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]){
   await db.query('UPDATE storefront_settings SET theme_id=$1',[theme]);
   await db.query('UPDATE administrators SET locale=$1',[locale]);
   const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});await context.addCookies(cookies);const page=await context.newPage(),errors=[];page.on('pageerror',error=>errors.push(error.message));
   try{
    fs.rmSync(path.join(updateRoot,'requests/install.json'),{force:true});writeStatus('available');
    await page.goto(origin+'/admin');
    const maintenance=page.locator('.admin-sidebar details').filter({has:page.locator('summary', {hasText:locale==='it'?'Manutenzione':'Maintenance'})});
    await maintenance.locator('summary').focus();await maintenance.locator('summary').press('Enter');
    const link=page.locator('.admin-sidebar a[href="/admin/updates"]');await link.focus();await link.press('Enter');await page.waitForURL('**/admin/updates');
    assert.equal(await page.locator('h1').innerText(),locale==='it'?'Aggiornamenti':'Updates');
    assert.ok(await page.locator('main').innerText().then(t=>t.includes('999.0.0')));
    const table=page.getByRole('table');
    assert.equal(await table.locator('tbody tr').count(),3);
    assert.ok(await table.innerText().then(t=>t.includes('koala-import')&&t.includes('vendor-inventory')&&t.includes(locale==='it'?'Resta disattivato':'Stays disabled')));
    const form=page.locator('form[action="/admin/updates"]');const checkbox=form.locator('input[name=confirm]');
    await checkbox.focus();await checkbox.press('Space');assert.equal(await checkbox.isChecked(),true);await checkbox.press('Tab');
    assert.equal(await form.locator('button').evaluate(e=>e===document.activeElement),true);
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    assert.ok(await table.evaluate(t=>t.getBoundingClientRect().right<=t.parentElement.getBoundingClientRect().right+1),'Component decisions and versions fit the mobile table');
    await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-available.png`),fullPage:true});
    await form.locator('button').press('Enter');await page.waitForURL('**/admin/updates');
    await page.getByRole('heading',{name:locale==='it'?'Aggiornamento richiesto':'Update requested',exact:true}).waitFor();
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(updateRoot,'requests/install.json'),'utf8')),candidate);
    assert.equal(await page.locator('form[action="/admin/updates"]').count(),0);
    fs.rmSync(path.join(updateRoot,'requests/install.json'));writeStatus('succeeded');
    await page.getByRole('link',{name:locale==='it'?'Ricarica stato':'Reload status',exact:true}).click();
    await page.getByRole('heading',{name:locale==='it'?'Aggiornamento completato':'Update completed',exact:true}).waitFor();
    assert.equal(await page.locator('form[action="/admin/updates"]').count(),0);
    assert.ok(await page.getByRole('table').locator('thead').innerText().then(t=>t.includes(locale==='it'?'Prima':'Before')),'Completed plan distinguishes previous versions from installed ones');
    writeStatus('blocked',{plan:{...plan,blocked:true,modules:[{...plan.modules[0],action:'blocked',blocker:'incompatible'},plan.modules[1]]}});
    await page.reload();
    assert.equal(await page.locator('form[action="/admin/updates"]').count(),0);
    await page.getByRole('heading',{name:locale==='it'?'Aggiornamento bloccato':'Update blocked',exact:true}).waitFor();
    await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-blocked.png`),fullPage:true});
    writeStatus('blocked',{candidate:null,notice:'engine_failed',plan:{...plan,blocked:true}});await page.reload();
    assert.equal(await page.locator('form[action="/admin/updates"]').count(),0,'Rejected root engine bytes cannot be reconfirmed');
    assert.ok((await page.locator('main').innerText()).includes(locale==='it'?'Il servizio di aggiornamento non ha superato':'The update service failed its self-check.'),'Engine self-check failure is localized');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    if(theme==='default'&&profile==='mobile')await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-engine-blocked.png`),fullPage:true});
    writeStatus('failed',{notice:'preparation_failed'});await page.reload();
    assert.equal(await page.locator('form[action="/admin/updates"]').count(),1,'Safe failures can be retried with a fresh root revalidation');
    for(const [code,it,en] of [
     ['busy','L’applicazione o un modulo è occupato.','The application or a module is busy.'],
     ['conflict','Lo stato dei componenti installati è cambiato.','The installed component state changed.'],
     ['configuration','La configurazione in esecuzione è cambiata.','The running configuration changed.'],
     ['unavailable','L’applicazione non ha completato la richiesta','The application could not complete the update control request.']]){
     writeStatus('failed',{notice:'control_'+code});await page.reload();
     assert.ok((await page.locator('main').innerText()).includes(locale==='it'?it:en),'Typed control refusal has a localized reason');
     assert.equal(await page.locator('form[action="/admin/updates"]').count(),1,'Retry still requires a fresh root revalidation');
     assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
     if(code==='busy'&&theme==='default'&&profile==='mobile')await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-busy.png`),fullPage:true});
    }
    writeStatus('current',{candidate:null});await page.reload();
    const check=page.locator('form[action="/admin/updates/check"] button');
    assert.equal((await check.innerText()).trim(),locale==='it'?'Controlla ora':'Check now');
    await check.focus();await check.press('Enter');await page.locator('#update-check-queued').waitFor();
    assert.ok(fs.existsSync(path.join(updateRoot,'requests/check.json')),'Manual check marker published');
    assert.equal(await page.locator('form[action="/admin/updates/check"]').count(),0,'A queued check cannot be requested twice');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    if(theme==='default')await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-check-queued.png`),fullPage:true});
    fs.rmSync(path.join(updateRoot,'requests/check.json'));
    writeStatus('failed',{recovery_required:true});await page.reload();
    assert.equal(await page.locator('form[action="/admin/updates"]').count(),0);
    assert.equal(await page.locator('form[action="/admin/updates/check"]').count(),0,'Recovery hides the manual check');
    await page.getByRole('heading',{name:locale==='it'?'Intervento sul server necessario':'Server intervention required',exact:true}).waitFor();
    if(theme==='default'&&locale==='en'&&profile==='desktop'){
     fs.writeFileSync(path.join(updateRoot,'status.json'),JSON.stringify({format:1,phase:'available',checked_at:'2026-11-01T10:00:00+00:00',candidate}));
     await page.reload();assert.equal(await page.locator('form[action="/admin/updates"]').count(),1,'Legacy status stays supported');
     assert.equal(await page.getByRole('table').count(),0);
    }
    assert.deepEqual(errors,[]);
    results.push({theme,locale,profile,passed:true});console.log(theme,locale,profile,'passed');
   }finally{await context.close();}
  }
  await setup.close();fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
 }finally{await stop();await browser.close();await db.query('DROP SCHEMA IF EXISTS '+schema+' CASCADE');await db.end();fs.rmSync(folder,{recursive:true,force:true});}
})().catch(e=>{console.error(e.stack);process.exitCode=1;});
