// Package update/uninstall through the real base and signed synthetic fixture executables.
// Isolated PostgreSQL schema, private temporary folder; no application database or storage.
const assert=require('node:assert/strict');
const fs=require('node:fs');const os=require('node:os');const path=require('node:path');const net=require('node:net');
const {spawn,spawnSync}=require('node:child_process');const {Client}=require('pg');const {chromium}=require('playwright');
const delay=ms=>new Promise(r=>setTimeout(r,ms));
(async()=>{
 const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);
 assert.ok(['localhost','127.0.0.1','[::1]'].includes(url.hostname)&&url.pathname.endsWith('_test'),'Dedicated local _test DB required');
 const base=path.resolve(process.env.TALOCART_BASE_TEST_BINARY||'target/debug/talocart');
 const fixtures=path.resolve(process.env.TALOCART_FIXTURE_DIR||'target/debug');
 const folder=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'talocart-package-updates-')));fs.chmodSync(folder,0o700);
 const artifacts=path.resolve('.local/verification/package-updates/browser');fs.mkdirSync(artifacts,{recursive:true});
 const owner=new Client({connectionString:url.href});await owner.connect();
 const schema=`talocart_package_updates_${process.pid}_${Date.now()}`;let client,server,browser;
 const config=path.join(folder,'talocart.toml');
 const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('TALOCART_')));
 const run=(args)=>{const result=spawnSync(base,args,{env,cwd:folder,encoding:'utf8',timeout:180000});assert.equal(result.status,0,`CLI ${args[0]} failed (private output omitted)`);return result.stdout.trim();};
 const freePort=()=>new Promise((resolve,reject)=>{const s=net.createServer();s.on('error',reject);s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>resolve(p));});});
 const stop=async()=>{if(!server)return;const child=server;server=null;child.kill('SIGTERM');for(let i=0;i<100;i++){if(child.exitCode!==null||child.signalCode!==null)return;await delay(100);}child.kill('SIGKILL');throw new Error('Synthetic host did not stop gracefully');};
 try {
  await owner.query(`CREATE SCHEMA ${schema}`);url.searchParams.set('options',`-csearch_path=${schema}`);client=new Client({connectionString:url.href});await client.connect();
  const port=await freePort();const origin=`http://127.0.0.1:${port}`;
  fs.mkdirSync(path.join(folder,'media'));fs.writeFileSync(config,`[server]\nlisten="127.0.0.1:${port}"\n[database]\nurl=${JSON.stringify(url.href)}\n[media]\nroot=${JSON.stringify(path.join(folder,'media'))}\n[packages]\ninstall="panel"\nroot=${JSON.stringify(path.join(folder,'packages'))}\n`,{mode:0o600});
  run(['--config',config,'migrate']);
  const pkg={};
  for(const name of ['v1','v2','v3-failing','v3-fixed']){const native=path.join(folder,`fixture-${name}`);fs.copyFileSync(path.join(fixtures,`fixture-${name}`),native);fs.chmodSync(native,0o700);pkg[name]=path.join(folder,`fixture-${name}.talopkg`);run(['package-build','--executable',native,'--output',pkg[name]]);}
  const start=async(degraded=false)=>{
   const log=fs.openSync(path.join(folder,'host.log'),'a');server=spawn(base,['--config',config,'serve'],{env,cwd:folder,stdio:['ignore',log,log]});fs.closeSync(log);
   for(let i=0;i<180;i++){assert.equal(server.exitCode,null,'Synthetic host exited');try{if((await fetch(origin+(degraded?'/health/live':'/health/ready'))).status===200)return;}catch{}await delay(250);}throw new Error('Synthetic host readiness timeout');
  };
  await start();
  let r=await fetch(origin+'/setup');let html=await r.text();let cookie=r.headers.get('set-cookie').split(';')[0];let csrf=html.match(/name="csrf" value="([^"]+)"/)[1];
  r=await fetch(origin+'/setup',{method:'POST',redirect:'manual',headers:{cookie,'content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams({csrf,email:'owner@example.test',password:'synthetic package password',name:'Package test',display_name:'Owner',primary_locale:'en',locale:'en'})});assert.equal(r.status,303,'Synthetic bootstrap');
  r=await fetch(origin+'/admin/login');html=await r.text();cookie=r.headers.getSetCookie().find(v=>v.startsWith('talocart_session=')).split(';')[0];csrf=html.match(/name="csrf" value="([^"]+)"/)[1];
  r=await fetch(origin+'/admin/login',{method:'POST',redirect:'manual',headers:{cookie,'content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams({csrf,email:'owner@example.test',password:'synthetic package password'})});assert.equal(r.status,303,'Synthetic owner login');cookie=r.headers.getSetCookie().find(v=>v.startsWith('talocart_session=')).split(';')[0];
  const get=async(pathname)=>{const response=await fetch(origin+pathname,{headers:{cookie},redirect:'manual'});return {response,html:await response.text()};};
  ({html}=await get('/admin/modules'));csrf=html.match(/name="csrf" value="([^"]+)"/)[1];
  const post=async(pathname,body)=>fetch(origin+pathname,{method:'POST',redirect:'manual',headers:{cookie,'content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams({...(/^\/admin\/modules\/(install|update|retry)$/.test(pathname)?{password:'synthetic package password'}:{}),...body})});
  const upload=async(file)=>{const form=new FormData();form.set('csrf',csrf);form.set('package',new Blob([fs.readFileSync(file)]),'fixture.talopkg');const response=await fetch(origin+'/admin/modules/preview',{method:'POST',headers:{cookie},body:form});return {response,html:await response.text()};};
  const ledger=async()=>(await client.query('SELECT version FROM _fixture_module_migrations ORDER BY version')).rows.map(row=>Number(row.version));
  assert.ok(!html.includes('package-public-key'), 'No trust-key form is required');
  // The server policy rejects direct POSTs, including multipart preview, with no registry change.
  await stop();const panelConfig=fs.readFileSync(config,'utf8');
  fs.writeFileSync(config,panelConfig.replace('install="panel"','install="cli"'));
  await start();
  assert.equal((await upload(pkg.v1)).response.status,403);
  for(const action of ['install','update','retry']) assert.equal((await post('/admin/modules/'+action,{csrf,preview:'invalid',id:'fixture-module'})).status,403);
  assert.ok(!(await get('/admin/modules')).html.includes('id="package-file"'));
  await stop();fs.writeFileSync(config,panelConfig);await start();
  let preview=await upload(pkg.v1);assert.equal(preview.response.status,200);assert.ok(preview.html.includes('data-preview="install"'));
  const initialToken=preview.html.match(/name="preview" value="([^"]+)"/)[1];
  for(const password of ['', 'wrong']) assert.equal((await post('/admin/modules/install',{csrf,preview:initialToken,password})).status,422);
  assert.ok(!(await get('/admin/modules')).html.includes('data-package="fixture-module"'));
  assert.equal((await post('/admin/modules/install',{csrf,preview:preview.html.match(/name="preview" value="([^"]+)"/)[1]})).status,303);
  assert.equal((await post('/admin/modules/toggle',{csrf,id:'fixture-module',enabled:'true'})).status,303);
  assert.match((await get('/admin/extensions/fixture-module')).html,/fixture-module 1\.0\.0/);
  browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
  const results=[];
  const open=async(width,js)=>{const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});await context.addCookies([{name:'talocart_session',value:cookie.split('=')[1],url:origin}]);const page=await context.newPage();page.setDefaultTimeout(120000);const errors=[];page.on('pageerror',e=>errors.push(e.message));return {context,page,errors};};
  const select=async(page,file)=>{await page.goto(origin+'/admin/modules');await page.locator('#package-file').setInputFiles(file);await page.locator('form[action="/admin/modules/preview"] button').click();await page.waitForSelector('#package-preview-title');};
  // Update preview in every language/layout/JavaScript profile; nothing is confirmed here.
  for(const [profile,width,js] of [['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]) {
   for(const locale of ['en','it']) {
    await client.query('UPDATE administrators SET locale=$1',[locale]);
    const {context,page,errors}=await open(width,js);
    try {
     await page.goto(origin+'/admin/modules');
     const policy=page.locator('form[action="/admin/modules/critical"]');
     await policy.locator('select').selectOption('false');
     await policy.locator('button').click();
     await page.waitForURL('**/admin/modules');
     assert.equal(await page.locator('[data-package="fixture-module"]').getAttribute('data-critical'),'false');
     assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
     await page.screenshot({path:path.join(artifacts,`critical-${profile}-${locale}.png`),fullPage:true});
     await select(page,pkg.v2);
     assert.equal(await page.locator('html').getAttribute('lang'),locale);
     assert.equal(await page.locator('[data-preview]').getAttribute('data-preview'),'update');
     assert.equal(await page.locator('#package-update-from').innerText(),'1.0.0');assert.equal(await page.locator('#package-update-to').innerText(),'1.1.0');
     assert.equal(await page.locator('#package-update-migrations').innerText(),locale==='it'?'1 nuova migrazione':'1 new migration');
     assert.equal(await page.locator('form[action="/admin/modules/update"] button').innerText(),locale==='it'?'Conferma aggiornamento':'Confirm update');
     assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'No horizontal overflow');
     await page.screenshot({path:path.join(artifacts,`update-preview-${profile}-${locale}.png`),fullPage:true});
     assert.deepEqual(errors,[]);results.push({profile,locale,passed:true});
    }finally{await context.close();}
   }
  }
  assert.deepEqual(await ledger(),[1],'Previews never migrate');
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder,'packages/registry.json'))).installed[0].critical,false,'Policy persisted');
  // Confirm with the keyboard and no JavaScript.
  await client.query('UPDATE administrators SET locale=$1',['en']);
  let session=await open(1440,false);
  await select(session.page,pkg.v2);const token=await session.page.locator('input[name=preview]').inputValue();
  await session.page.locator('form[action="/admin/modules/update"] input[name="password"]').fill('synthetic package password');
  await session.page.locator('form[action="/admin/modules/update"] button').focus();await session.page.keyboard.press('Enter');await session.page.waitForURL('**/admin/modules');
  const card=session.page.locator('[data-package="fixture-module"]');
  assert.match(await card.locator('h3').innerText(),/1\.1\.0/);assert.equal(await card.locator('[data-previous]').innerText(),'1.0.0');assert.equal(await card.getAttribute('data-state'),'ready');
  await session.page.screenshot({path:path.join(artifacts,'updated-en.png'),fullPage:true});await session.context.close();
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder,'packages/registry.json'))).installed[0].critical,false,'Update preserves owner policy');
  assert.deepEqual(await ledger(),[1,2]);assert.match((await get('/admin/extensions/fixture-module')).html,/fixture-module 1\.1\.0/);
  assert.equal((await post('/admin/modules/update',{csrf,preview:token})).status,422,'Replay rejected');
  preview=await upload(pkg.v1);assert.equal(preview.response.status,422,'Downgrade rejected');assert.ok(preview.html.includes('id="package-error"'));
  // A failing migration leaves a visible incomplete state with retry, then a corrected package completes.
  for(const locale of ['en','it']){
   await client.query('UPDATE administrators SET locale=$1',[locale]);
   session=await open(390,true);
   await select(session.page,locale==='en'?pkg['v3-failing']:pkg['v3-fixed']);
   if(locale==='en'){
  await session.page.locator('form[action="/admin/modules/update"] input[name="password"]').fill('synthetic package password');
    await session.page.locator('form[action="/admin/modules/update"] button').click();await session.page.waitForSelector('#package-error');
    assert.equal(await session.page.locator('[data-package="fixture-module"]').getAttribute('data-state'),'incomplete');
    assert.equal(await session.page.locator('form[action="/admin/modules/retry"] button').innerText(),'Retry update');
    // A damaged pending executable is isolated and must not offer a futile retry.
    await stop();
    const inventory=JSON.parse(fs.readFileSync(path.join(folder,'packages/registry.json'),'utf8'));
    const pending=inventory.installed[0].update.manifest;
    const pendingPath=path.join(folder,'packages/installed',pending.id,pending.version,'module');
    const savedExecutable=fs.readFileSync(pendingPath);fs.unlinkSync(pendingPath);
    await start(true);
    assert.equal((await fetch(origin+'/health/ready')).status,200,'Optional damaged module leaves the store ready');
    assert.equal((await fetch(origin+'/health/ready?all_modules=true')).status,503,'Strict readiness rejects damaged optional module');
    for(const damagedLocale of ['en','it']) {
      await client.query('UPDATE administrators SET locale=$1',[damagedLocale]);
      const response=await get('/admin/modules');assert.equal(response.response.status,200);
      assert.ok(response.html.includes('data-state="damaged"'));
      assert.ok(response.html.includes(damagedLocale==='en'?'Damaged':'Danneggiato'));
      assert.ok(!response.html.includes('action="/admin/modules/retry"'));
      await session.page.goto(origin+'/admin/modules');
      await session.page.screenshot({path:path.join(artifacts,`damaged-${damagedLocale}.png`),fullPage:true});
    }
    await stop();fs.writeFileSync(pendingPath,savedExecutable,{mode:0o700});
    await client.query('UPDATE administrators SET locale=$1',['en']);await start(true);
    await session.page.goto(origin+'/admin/modules');
    assert.equal((await get('/admin/extensions/fixture-module')).response.status,503,'Incomplete module is not served');
    assert.equal((await fetch(origin+'/admin/extensions/fixture-module',{redirect:'manual'})).status,303,'Anonymous callers are sent to login before availability is revealed');
    // A blocked inspection remains reportable even when the target DB cannot be opened.
    const unavailable=path.join(folder,'unavailable-plan.toml');
    fs.writeFileSync(unavailable,fs.readFileSync(config,'utf8').replace(JSON.stringify(url.href),JSON.stringify('postgres://synthetic@127.0.0.1:1/unavailable_test')),{mode:0o600});
    const blocked=spawnSync(base,['--config',unavailable,'package-update-plan','--check-database'],{env,cwd:folder,encoding:'utf8',timeout:10000});
    assert.equal(blocked.status,1);const report=JSON.parse(blocked.stdout);
    assert.equal(report.database_checked,false);assert.equal(report.modules[0].blocker,'incomplete');
    await session.page.screenshot({path:path.join(artifacts,'incomplete-mobile-en.png'),fullPage:true});
  await session.page.locator('form[action="/admin/modules/retry"] input[name="password"]').fill('synthetic package password');
    await session.page.locator('form[action="/admin/modules/retry"] button').click();await session.page.waitForSelector('#package-error');
    assert.deepEqual(await ledger(),[1,2,3],'Committed migrations are kept, never reverted');
   }else{
    assert.ok((await session.page.locator('[data-package="fixture-module"]').innerText()).includes('L’aggiornamento non è stato completato'));
  await session.page.locator('form[action="/admin/modules/update"] input[name="password"]').fill('synthetic package password');
    await session.page.locator('form[action="/admin/modules/update"] button').click();await session.page.waitForURL('**/admin/modules');
    assert.equal(await session.page.locator('[data-package="fixture-module"]').getAttribute('data-state'),'ready');
   }
   assert.deepEqual(session.errors,[]);await session.context.close();
  }
  assert.deepEqual(await ledger(),[1,2,3,4]);assert.match((await get('/admin/extensions/fixture-module')).html,/fixture-module 1\.2\.1/);
  // Uninstall is separate, needs an inactive module and keeps schema and data.
  assert.equal((await post('/admin/modules/uninstall',{csrf,id:'fixture-module'})).status,422,'Active module cannot be uninstalled');
  await client.query("INSERT INTO fixture_module_notes(body) VALUES('kept')");
  assert.equal((await post('/admin/modules/toggle',{csrf,id:'fixture-module',enabled:'false'})).status,303);
  await client.query('UPDATE administrators SET locale=$1',['it']);
  session=await open(390,false);await session.page.goto(origin+'/admin/modules');
  const summary=session.page.locator('[data-package="fixture-module"] .package-uninstall summary');assert.equal(await summary.innerText(),'Disinstalla');
  await summary.focus();await session.page.keyboard.press('Enter');
  assert.ok(await session.page.locator('[data-package="fixture-module"] .package-uninstall p').isVisible());
  await session.page.screenshot({path:path.join(artifacts,'uninstall-mobile-no-js-it.png'),fullPage:true});
  await session.page.keyboard.press('Tab');assert.equal(await session.page.evaluate(()=>document.activeElement.textContent),'Conferma disinstallazione');
  const uninstalled=session.page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname==='/admin/modules/uninstall');
  await session.page.keyboard.press('Enter');assert.equal((await uninstalled).status(),303);
  await session.page.locator('[data-package="fixture-module"]').waitFor({state:'detached'});
  await session.page.waitForURL('**/admin/modules');
  assert.equal(await session.page.locator('[data-package="fixture-module"]').count(),0);await session.context.close();
  assert.ok(!fs.existsSync(path.join(folder,'packages/installed/fixture-module')));assert.ok(fs.existsSync(path.join(folder,'packages/data/fixture-module')));
  assert.deepEqual(await ledger(),[1,2,3,4]);assert.equal(Number((await client.query('SELECT count(*) FROM fixture_module_notes')).rows[0].count),1);
  assert.equal((await get('/admin/extensions/fixture-module')).response.status,404);
  await stop();await start();assert.ok(!(await get('/admin/modules')).html.includes('data-package="fixture-module"'),'Uninstall persists across restart');
  // CLI-only sites can install, update and retry without reopening the panel.
  const cliSchema=schema+'_cli';await owner.query(`CREATE SCHEMA ${cliSchema}`);
  const cliUrl=new URL(url);cliUrl.searchParams.set('options',`-csearch_path=${cliSchema}`);
  const cliConfig=path.join(folder,'cli.toml');
  fs.writeFileSync(cliConfig,`[database]\nurl=${JSON.stringify(cliUrl.href)}\n[media]\nroot="cli-media"\n[packages]\nroot="cli-packages"\n`,{mode:0o600});
  const cliDb=new Client({connectionString:cliUrl.href});await cliDb.connect();
  try {
    run(['--config',cliConfig,'migrate']);
    run(['--config',cliConfig,'package-install','--package',pkg.v1,'--enable']);
    run(['--config',cliConfig,'package-update','--package',pkg.v2]);
    let registry=JSON.parse(fs.readFileSync(path.join(folder,'cli-packages/registry.json')));
    assert.equal(registry.installed[0].manifest.version,'1.1.0');assert.equal(registry.installed[0].enabled,true);
    const failed=spawnSync(base,['--config',cliConfig,'package-update','--package',pkg['v3-failing']],{env,cwd:folder,encoding:'utf8',timeout:180000});
    assert.equal(failed.status,1);
    await cliDb.query('CREATE TABLE fixture_module_missing(id BIGINT PRIMARY KEY)');
    run(['--config',cliConfig,'package-retry','--id','fixture-module']);
    registry=JSON.parse(fs.readFileSync(path.join(folder,'cli-packages/registry.json')));
    assert.equal(registry.installed[0].manifest.version,'1.2.0');assert.equal(registry.installed[0].enabled,true);assert.ok(!registry.installed[0].update);
  } finally { await cliDb.end();await owner.query(`DROP SCHEMA ${cliSchema} CASCADE`); }
  fs.writeFileSync(path.join(artifacts,'results.json'),JSON.stringify({results,update:true,replay_rejected:true,downgrade_rejected:true,incomplete_retry:true,uninstall_keeps_data:true},null,2));
  console.log(JSON.stringify({profiles:results.length,passed:true,isolated_schema:true}));
 } finally {
  const hostLog=path.join(folder,'host.log');if(fs.existsSync(hostLog))fs.writeFileSync(path.join(artifacts,'host-latest.log'),fs.readFileSync(hostLog,'utf8').replaceAll(url.href,'[isolated database]'));
  if(browser)await browser.close();await stop();if(client)await client.end();await owner.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);await owner.end();fs.rmSync(folder,{recursive:true,force:true});
 }
})().catch(e=>{console.error(e instanceof assert.AssertionError?e.stack:(e.message||'Package update verification failed'));process.exitCode=1;});
