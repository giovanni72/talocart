// Dedicated schemas and synthetic accounts only. Shared navigation across both themes.
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {randomBytes}=require('node:crypto'),{spawn,spawnSync}=require('node:child_process'),{once}=require('node:events');
const {Client}=require('pg'),{chromium}=require('playwright');
(async()=>{
 assert.ok(process.env.TALOCART_TEST_DATABASE_URL,'Dedicated test database required');
 const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 const schema='talocart_maintenance_'+process.pid+'_'+Date.now(),folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-maintenance-'));
 const origin='http://127.0.0.1:3142',output=path.resolve('.local/verification/maintenance/browser');fs.mkdirSync(output,{recursive:true});
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
  await admin.getByLabel('Nome del negozio',{exact:true}).fill('Maintenance preview');await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);
  await admin.getByRole('button',{name:'Crea negozio',exact:true}).click();await admin.waitForURL('**/admin/login');
  await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);await admin.getByRole('button',{name:'Accedi',exact:true}).click();await admin.waitForURL('**/admin');
  const cookies=(await setup.cookies()).filter(c=>c.name==='talocart_session');
  for(const theme of ['default','essential'])for(const locale of ['it','en'])for(const[profile,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]){
   await db.query('UPDATE storefront_settings SET theme_id=$1',[theme]);
   await db.query('UPDATE administrators SET locale=$1',[locale]);
   const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js,colorScheme:"dark"});await context.addCookies(cookies);const page=await context.newPage(),errors=[];page.on('pageerror',error=>errors.push(error.message));
   try{
    await page.goto(origin+'/admin/settings/maintenance');
    assert.equal(await page.locator('h1').innerText(),locale==='it'?'Manutenzione del sito':'Site maintenance');
    await page.getByRole('button',{name:locale==='it'?'Metti il sito in manutenzione':'Enable site maintenance',exact:true}).focus();
    await page.keyboard.press('Enter');
    await page.getByRole('button',{name:locale==='it'?'Riapri il negozio':'Reopen the store',exact:true}).waitFor();
    assert.equal((await fetch(origin+'/maintenance/status')).status,503);
    const visitor=await context.newPage();visitor.on('pageerror',error=>errors.push(error.message));
    const result=await visitor.goto(origin+'/');assert.equal(result.status(),503);
    assert.equal(await visitor.locator('h1').innerText(),'Il negozio è in manutenzione.');
    const surface=()=>visitor.evaluate(()=>({background:getComputedStyle(document.body).backgroundColor,color:getComputedStyle(document.body).color,scheme:getComputedStyle(document.documentElement).colorScheme}));
    const darkPreference=await surface();assert.equal(darkPreference.scheme,'light');
    await visitor.emulateMedia({colorScheme:'light'});assert.deepEqual(await surface(),darkPreference,'Light page stays the same under either OS theme');
    await visitor.emulateMedia({colorScheme:'dark'});
    assert.ok(await visitor.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    await visitor.getByRole('link',{name:'Ricarica / Reload',exact:true}).focus();
    await visitor.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-maintenance.png'),fullPage:true,animations:'disabled'});
    await page.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-settings.png'),fullPage:true,animations:'disabled'});
    assert.equal((await fetch(origin+'/admin/login')).status,200);
    await stop();await start();
    assert.equal((await fetch(origin+'/')).status,503,'Manual closure persists after restart');
    await page.reload();
    await page.getByRole('button',{name:locale==='it'?'Riapri il negozio':'Reopen the store',exact:true}).click();
    await page.getByRole('button',{name:locale==='it'?'Metti il sito in manutenzione':'Enable site maintenance',exact:true}).waitFor();
    assert.equal((await fetch(origin+'/maintenance/status')).status,204);
    assert.equal((await fetch(origin+'/')).status,200);
    if(theme==='default'&&locale==='it'&&profile==='desktop') {
      await visitor.waitForResponse(r=>r.url().endsWith('/maintenance/status')&&r.status()===204,{timeout:20000});
      await visitor.waitForFunction(()=>!document.querySelector('.k'),undefined,{timeout:5000});
    } else await visitor.reload();
    assert.equal(await visitor.locator('main').count(),1);
    await visitor.close();assert.deepEqual(errors,[]);
    results.push({theme,locale,profile,passed:true});console.log(theme,locale,profile,'passed');
   }finally{await context.close();}
  }
  await setup.close();fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
 }finally{await stop();await browser.close();await db.query('DROP SCHEMA IF EXISTS '+schema+' CASCADE');await db.end();fs.rmSync(folder,{recursive:true,force:true});}
})().catch(e=>{console.error(e.stack);process.exitCode=1;});
