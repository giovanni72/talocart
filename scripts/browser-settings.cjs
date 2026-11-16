// Unified settings: real native forms, disposable PostgreSQL schema and synthetic media.
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {randomBytes}=require('node:crypto'),{spawn,spawnSync}=require('node:child_process'),{once}=require('node:events'),{Client}=require('pg'),{chromium}=require('playwright');
(async()=>{
 assert.ok(process.env.TALOCART_TEST_DATABASE_URL,'Dedicated test DB required');
 const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 const output=path.resolve('.local/verification/settings');fs.mkdirSync(output,{recursive:true});const results=[];
 try{for(const[name,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]){
 const schema=`talocart_settings_${process.pid}_${Date.now()}`,folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-settings-')),origin='http://127.0.0.1:3117';
 const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options',`-csearch_path=${schema}`);
 const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3117"\n',{mode:0o600});
 const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3117',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'},binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart');let server;
 const query=(sql,args)=>db.query(sql,args);
 const start=async()=>{const fd=fs.openSync(path.join(output,`${name}-server.log`),'a',0o600);server=spawn(binary,['--config',config,'serve'],{env,stdio:['ignore',fd,fd]});fs.closeSync(fd);for(let i=0;i<100;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Readiness failed');};
 const stop=async()=>{if(server&&server.exitCode===null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
 const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js}),page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
 try{
  await query(`CREATE SCHEMA ${schema}`);await query(`SET search_path TO ${schema}`);
  const migration=spawnSync(binary,['--config',config,'migrate'],{env,encoding:'utf8'});assert.equal(migration.status,0,'Isolated migrate');await start();
  await context.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);const password=randomBytes(20).toString('hex');
  await page.goto(origin+'/setup');await page.getByLabel('Nome del negozio',{exact:true}).fill('Settings test');await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);await page.getByRole('button',{name:'Crea negozio',exact:true}).click();await page.waitForURL('**/admin/login');await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);await page.getByRole('button',{name:'Accedi',exact:true}).click();await page.waitForURL('**/admin');
  await page.goto(origin+'/admin/media');const csrf=await page.locator('input[name=csrf]').first().inputValue();
  for(let i=0;i<41;i++){
   const upload=await context.request.post(origin+'/admin/media/upload',{multipart:{csrf,images:{name:`image-${String(i).padStart(2,'0')}.png`,mimeType:'image/png',buffer:fs.readFileSync('crates/web/tests/fixtures/avatar.png')}},maxRedirects:0});assert.equal(upload.status(),303,'Synthetic image upload');
  }
  const media=(await query("SELECT m.id FROM media_objects m JOIN media_library l ON l.key=m.key WHERE l.name='image-40.png'")).rows[0].id;
  const submit=async(action,status=200)=>{const done=page.waitForResponse(r=>r.request().method()==='POST'&&['/admin/settings/save','/admin/media'].includes(new URL(r.url()).pathname));await page.locator(`button[name=action][value="${action}"]:visible`).click();assert.equal((await done).status(),status,action);await page.waitForLoadState('load');};
  const tab=async id=>{await page.locator(`#settings-tab-${id}`).click();if(js)assert.equal(await page.locator(`#${id}`).isVisible(),true);};
  const readSaved=async()=>{const row=(await query('SELECT name,logo_key,favicon_key FROM store_settings')).rows[0];return row;};
  for(const language of ['it','en']){
   await query('UPDATE administrators SET locale=$1',[language]);await page.goto(origin+'/admin/settings');
   assert.equal(await page.locator('#settings-save').count(),1);if(js)assert.equal(await page.locator('#settings-change-status').innerText(),language==='it'?'Nessuna modifica da salvare':'No unsaved changes');
   const title=`Draft ${name} ${language}`;await page.locator('#store-name').fill(title);await page.locator('#store-city').fill('Roma');
   await tab('storefront');const selectedTheme=language==='it'?'essential':'default';await page.locator('#theme-id').selectOption(selectedTheme);await page.locator('#theme-palette').selectOption(language==='it'?'clay':'forest');await page.locator('#theme-seo_title').fill(`SEO ${language}`);
   const indexed=language==='it',indexing=page.getByRole('checkbox',{name:language==='it'?'Consenti l’indicizzazione del negozio':'Allow search engines to index the store',exact:true});
   assert.equal(await indexing.isChecked(),language==='en');await indexing.focus();await page.keyboard.press('Space');assert.equal(await indexing.isChecked(),indexed);
   if(js)assert.equal(await page.locator('#settings-tab-storefront [data-tab-dirty]').isVisible(),true);
   if (!process.env.TALOCART_TEST_NO_SCREENSHOTS) await page.screenshot({path:path.join(output,`${name}-${language}-indexing.png`),fullPage:true});
   await tab('general');assert.equal(await page.locator('#store-name').inputValue(),title);
   if(js){await page.locator('#settings-tab-general').focus();await page.keyboard.press('ArrowRight');assert.equal(await page.locator('#appearance').isVisible(),true);assert.equal(await page.locator('#settings-tab-general [data-tab-dirty]').isVisible(),true);}
   await tab('appearance');const before=await readSaved();await submit('browse_logo');assert.equal(await page.locator('#media-draft [name=name]').inputValue(),title);
   assert.equal(await page.locator('.settings-media-choice').count(),40);await submit('media_next');assert.equal(await page.locator('.settings-media-choice').count(),1);await submit(`choose_${media}`);
   assert.equal((await readSaved()).logo_key,before.logo_key);assert.equal(await page.locator('#settings-logo').inputValue(),String(media));
   await submit('browse_favicon');await page.locator('#settings-media-query').fill('image-40');await submit('media_search');assert.equal(await page.locator('.settings-media-choice').count(),1);await submit(`choose_${media}`);
   assert.deepEqual(await readSaved(),before,'Picker never applies settings');await tab('countries');await page.locator('#checkout-countries').selectOption(['IT','FR']);
   await tab('general');
   assert.equal(await page.locator('#settings-maintenance').getAttribute('type'),'checkbox');
   await page.locator('#settings-maintenance').check();await page.locator('#module-install-mode').selectOption('panel');await page.locator('#module-install-password').fill(password);
   await Promise.all([page.waitForNavigation({waitUntil:'load'}),submit('save',303)]);
   assert.equal((await query('SELECT maintenance_enabled FROM storefront_settings')).rows[0].maintenance_enabled,true);
   assert.equal((await query('SELECT module_install_mode FROM store_settings')).rows[0].module_install_mode,'panel');
   assert.equal((await context.request.get(origin+'/')).status(),503);
   assert.equal(await page.locator('#module-install-password').inputValue(),'');
   await page.locator('#settings-maintenance').uncheck();await Promise.all([page.waitForNavigation({waitUntil:'load'}),submit('save',303)]);
   assert.equal((await query('SELECT maintenance_enabled FROM storefront_settings')).rows[0].maintenance_enabled,false);
   await tab('general');await page.locator('#platform-time-zone').selectOption('UTC');await page.locator('#platform-date-format').selectOption('ymd');
   if(js){await page.locator('#store-name').fill('');await tab('storefront');await page.locator('#settings-save').click();assert.equal(await page.locator('#general').isVisible(),true);assert.equal(await page.locator('#store-name').evaluate(e=>e===document.activeElement),true);await page.locator('#store-name').fill(title);assert.equal(await page.locator('#settings-tab-general [data-tab-error]').isVisible(),false,'Corrected native validation clears the tab error');}
   await tab('appearance');if (!process.env.TALOCART_TEST_NO_SCREENSHOTS) await page.screenshot({path:path.join(output,`${name}-${language}-draft.png`),fullPage:true});await submit('save',303);await page.waitForURL('**/admin/settings/appearance?saved=1');
   const saved=await readSaved();assert.equal(saved.name,title);assert.ok(saved.logo_key&&saved.favicon_key);
   assert.equal((await query('SELECT theme_id,indexing_enabled FROM storefront_settings')).rows[0].theme_id,selectedTheme);assert.equal((await query('SELECT indexing_enabled FROM storefront_settings')).rows[0].indexing_enabled,indexed);
   await stop();await start();
   const primaryLocale=(await query('SELECT primary_locale FROM storefront_settings')).rows[0].primary_locale;
   const publicPage=await context.request.get(origin+(language===primaryLocale?'':'/'+language));
   assert.equal(publicPage.status(),200);assert.equal(publicPage.headers()['x-robots-tag'],indexed?'index, follow':'noindex, nofollow');const robots=await(await context.request.get(origin+'/robots.txt')).text();assert.ok(robots.includes('Disallow:\n'));assert.equal(robots.includes('Sitemap:'),indexed);assert.ok((await publicPage.text()).includes(`data-theme="${selectedTheme}"`),'Saved theme survives a process restart');
   await page.goto(origin+'/admin/settings/appearance?saved=1');
   const prefs=(await query('SELECT palette,seo_title FROM storefront_settings')).rows[0];assert.equal(prefs.palette,language==='it'?'clay':'forest');assert.equal(prefs.seo_title,`SEO ${language}`);
   assert.deepEqual((await query('SELECT country_code FROM checkout_countries ORDER BY country_code')).rows.map(r=>r.country_code),['FR','IT']);
   if(js)assert.equal(await page.locator('#settings-change-status').innerText(),language==='it'?'Nessuna modifica da salvare':'No unsaved changes');
   await submit('remove_logo');assert.equal((await context.request.get(new URL(await page.locator('.branding-preview-logo img').getAttribute('src'),origin).href)).status(),200,'Removal preview resolves');assert.equal((await readSaved()).logo_key,saved.logo_key);await submit('restore_logo');assert.equal(await page.locator('#settings-logo').inputValue(),'');
   await submit('remove_logo');await submit('remove_favicon');await submit('save',303);await page.waitForURL('**/admin/settings/appearance?saved=1');assert.equal((await readSaved()).logo_key,null);assert.equal((await readSaved()).favicon_key,null);
   await tab('general');await page.locator('#store-city').fill('Discard me');if(js){let warning;const seen=new Promise(resolve=>warning=resolve);page.once('dialog',async dialog=>{assert.equal(dialog.type(),'beforeunload');await dialog.dismiss();warning();});await page.locator('.admin-sidebar .wordmark').click();await seen;assert.ok(new URL(page.url()).pathname.startsWith('/admin/settings'));}if(js)page.once('dialog',dialog=>dialog.accept());await page.locator('[data-settings-discard]').click();await page.waitForURL('**/admin/settings');assert.equal(await page.locator('#store-city').inputValue(),'Roma');
   // Two browser editors: a stale submission retains all of its values and writes nothing.
   const other=await context.newPage();await other.goto(origin+'/admin/settings');await other.locator('#store-city').fill('Concurrent');await other.locator('#settings-save').click();await other.waitForURL('**/admin/settings?saved=1');await other.close();
   await page.locator('#store-name').fill('Stale draft');await submit('save',409);assert.equal(await page.locator('#store-name').inputValue(),'Stale draft');assert.equal((await readSaved()).name,title);
   if(js)page.once('dialog',dialog=>dialog.accept());await page.locator('[data-settings-discard]').click();await page.waitForURL('**/admin/settings');
   // Server validation in another panel returns the entire submitted form.
   const pairs=await page.locator('#settings-editor').evaluate(f=>[...new FormData(f)]);const body=new URLSearchParams(pairs);body.set('time_zone','Bad/Zone');body.set('name','Server draft');body.set('action','save');
   const invalid=await context.request.post(origin+'/admin/settings/save',{data:body.toString(),headers:{'content-type':'application/x-www-form-urlencoded'}});assert.equal(invalid.status(),422);assert.ok((await invalid.text()).includes('Server draft'));assert.equal((await query('SELECT indexing_enabled FROM storefront_settings')).rows[0].indexing_enabled,indexed);
   const preview=await context.request.get(origin+'/admin/settings/taxes/preview');assert.equal(preview.status(),200);assert.ok(!(await preview.text()).includes('name="action" value="save"'));
   for(const w of [320,390,651,1440]){await page.setViewportSize({width:w,height:1000});assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),`Overflow ${w}`);const box=await page.locator('.settings-savebar').boundingBox();assert.ok(box.x>=0&&box.x+box.width<=w+1);assert.ok(await page.locator('#settings-save > span').evaluate(label=>{const text=label.firstChild,range=document.createRange();range.setStart(text,text.textContent.lastIndexOf(' ')+1);range.setEnd(text,text.textContent.length);return range.getClientRects().length<=1;}),`Save label must not split a word at ${w}px`);}
   await page.setViewportSize({width,height:1000});
   // Capture the saved document after a complete load at the final viewport.
   // Rapid viewport changes can leave stale compositor tiles in a full-page capture.
   await page.reload({waitUntil:'load'});await page.evaluate(()=>window.scrollTo(0,0));
   assert.equal(await page.locator('.admin-shell').count(),1,'One administrator shell after resizing');
   assert.equal(await page.locator('#settings-save').count(),1,'One saved settings form');
   if (!process.env.TALOCART_TEST_NO_SCREENSHOTS) await page.screenshot({path:path.join(output,`${name}-${language}-saved.png`),fullPage:true});
  }

  // A pending class choice must remain in FormData after a concurrent deactivation.
  const savedDefaultClass=(await query('SELECT default_class_id FROM pricing_settings')).rows[0].default_class_id;
  const rate=(await query("INSERT INTO tax_rates(name,basis_points) VALUES('Concurrent VAT',2200) RETURNING id")).rows[0].id;
  const taxClass=(await query("INSERT INTO tax_classes(name,rate_id) VALUES('Concurrent class',$1) RETURNING id",[rate])).rows[0].id;
  await page.goto(origin+'/admin/settings/taxes');await page.locator('#default-class').selectOption(String(taxClass));
  await query('UPDATE tax_classes SET active=false WHERE id=$1',[taxClass]);await submit('save',409);
  assert.equal(await page.locator('#settings-editor').evaluate(f=>new FormData(f).get('default_class_id')),String(taxClass),'Concurrent deactivation must not clear the selected class');
  assert.equal((await query('SELECT default_class_id FROM pricing_settings')).rows[0].default_class_id,savedDefaultClass,'Rejected edits preserve the installed fiscal default');
  if(js)page.once('dialog',d=>d.accept());await page.locator('[data-settings-discard]').click();await page.waitForURL('**/admin/settings');
  // A mixed role keeps tax editing while all general fields remain read-only.
  const role='role_1234567890abcdef';
  await query('INSERT INTO admin_roles(key,name) VALUES($1,$2)',[role,'Tax editor']);
  for(const permission of ['settings.read','taxes.read','taxes.write'])await query('INSERT INTO admin_role_permissions(role_key,permission) VALUES($1,$2)',[role,permission]);
  const ownerCsrf=await page.locator('#settings-editor [name=csrf]').inputValue();
  const created=await context.request.post(origin+'/admin/users/new',{form:{csrf:ownerCsrf,email:'tax-editor@example.test',password,role},maxRedirects:0});assert.equal(created.status(),303);
  await query("UPDATE administrators SET locale='en' WHERE email='tax-editor@example.test'");
  const taxesContext=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js}),taxesPage=await taxesContext.newPage();
  taxesPage.on('pageerror',e=>errors.push(e.message));
  try{
    await taxesPage.goto(origin+'/admin/login');await taxesPage.locator('form[action="/admin/login"] [name=email]').fill('tax-editor@example.test');await taxesPage.locator('form[action="/admin/login"] [name=password]').fill(password);await taxesPage.locator('form[action="/admin/login"] button[type=submit]').click();await taxesPage.waitForURL('**/admin/settings');
    for(const locale of ['it','en']){
      await query('UPDATE administrators SET locale=$1 WHERE email=$2',[locale,'tax-editor@example.test']);await taxesPage.goto(origin+'/admin/settings');
      const cleanLabel=locale==='it'?'Nessuna modifica da salvare':'No unsaved changes',dirtyLabel=locale==='it'?'Modifiche non salvate':'Unsaved changes';
      assert.equal(await taxesPage.locator('#settings-save').count(),1);assert.equal(await taxesPage.locator('[name=name]').count(),0);
      assert.equal(await taxesPage.locator('#settings-readonly').innerText(),locale==='it'?'Le impostazioni generali sono in sola lettura; puoi modificare Prezzi e imposte.':'General settings are read-only; you can edit Prices and taxes.');
      if(js)assert.equal(await taxesPage.locator('#settings-change-status').innerText(),cleanLabel);
      await taxesPage.locator('#settings-tab-taxes').click();assert.equal(await taxesPage.locator('#store-currency').isEnabled(),true);
      const savedName=(await readSaved()).name,currency=locale==='it'?'USD':'EUR';await taxesPage.locator('#store-currency').selectOption(currency);
      if(js)assert.equal(await taxesPage.locator('#settings-change-status').innerText(),dirtyLabel);
      await taxesPage.locator('#settings-save').click();await taxesPage.waitForURL(u=>u.searchParams.get('saved')==='1');
      assert.equal((await query('SELECT currency FROM store_settings')).rows[0].currency,currency);assert.equal((await readSaved()).name,savedName);
      if(js)assert.equal(await taxesPage.locator('#settings-change-status').innerText(),cleanLabel);
      assert.ok(await taxesPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    }
  }finally{await taxesContext.close();}
  assert.deepEqual(errors,[]);results.push(`${name}: IT/EN tabs, keyboard, drafts, 40+1 media pagination/search, deferred branding, atomic save, both themes and indexing opt-in persisted across process restarts, keyboard indexing control and robots policy, native/server validation, discard, conflict, mixed tax permissions IT/EN, widths 320/390/651/1440 OK`);
 }finally{await context.close();await stop();await query('SET search_path TO public');await query(`DROP SCHEMA ${schema} CASCADE`);fs.rmSync(folder,{recursive:true,force:true});}
 }}finally{await browser.close();await db.end();}
 fs.writeFileSync(path.join(output,'results.txt'),results.join('\n')+'\n');console.log(results.join('\n'));
})().catch(e=>{console.error(e);process.exitCode=1;});
