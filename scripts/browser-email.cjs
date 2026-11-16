// Actual browser, isolated PostgreSQL schemas and Mailpit capture. No real recipients.
const assert=require('node:assert/strict');
const fs=require('node:fs');const os=require('node:os');const path=require('node:path');
const {randomBytes}=require('node:crypto');const {spawn,spawnSync}=require('node:child_process');const {once}=require('node:events');
const net=require('node:net');
const {Client}=require('pg');const {chromium}=require('playwright');
(async()=>{
 assert.ok(process.env.TALOCART_TEST_DATABASE_URL,'Dedicated test database required');
 const api=process.env.TALOCART_TEST_MAILPIT_URL||'http://127.0.0.1:8025';const smtpPort=Number(process.env.TALOCART_TEST_SMTP_PORT||1025);
 assert.equal(new URL(api).hostname,'127.0.0.1','Mailpit must be local');assert.ok((await fetch(api+'/api/v1/info')).ok,'Mailpit required');
 const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 const output=path.resolve('.local/verification/email');fs.mkdirSync(output,{recursive:true});const results=[];
 try{for(const[name,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]){
  const schema=`talocart_email_browser_${process.pid}_${Date.now()}`;const folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-email-'));
  const tag=randomBytes(6).toString('hex');const origin='http://127.0.0.1:3112';const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options',`-csearch_path=${schema}`);
  const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3112"\n',{mode:0o600});
  const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LISTEN:'127.0.0.1:3112',TALOCART_LOG:'warn'};
  const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart');let server;
  const cli=command=>{const r=spawnSync(binary,['--config',config,command],{env,encoding:'utf8'});assert.equal(r.status,0,`CLI ${command} failed (private output omitted)`);};
  const start=async()=>{const log=fs.openSync(path.join(output,`server-${name}.log`),'a',0o600);server=spawn(binary,['--config',config,'serve'],{env,stdio:['ignore',log,log]});fs.closeSync(log);for(let i=0;i<100;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Readiness failed');};
  const stop=async()=>{if(server&&server.exitCode===null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
  const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
  try{
   await db.query(`CREATE SCHEMA ${schema}`);await db.query(`SET search_path TO ${schema}`);cli('migrate');await start();
   const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));await context.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);
   await page.goto(origin+'/setup');const password=randomBytes(20).toString('hex');await page.getByLabel('Nome del negozio',{exact:true}).fill('Email preview');await page.getByLabel('Nome visualizzato',{exact:true}).fill('Test owner');await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);await page.getByRole('button',{name:'Crea negozio',exact:true}).click();await page.waitForURL('**/admin/login');await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);await page.getByRole('button',{name:'Accedi',exact:true}).click();await page.waitForURL('**/admin');
   const post=async(action,button,status=303)=>{const done=page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname===action);await page.locator(button).click();assert.equal((await done).status(),status);if(status===303)await page.waitForURL('**/admin/settings/email?notice=*');};
   const save=async(status=303)=>post('/admin/settings/email','form[action="/admin/settings/email"] button[type=submit]',status);
   const templates=['order_received','order_admin','payment_received','order_cancelled','cancellation_admin','payment_refunded'];
   for(const lang of['it','en']){
    await db.query('UPDATE administrators SET locale=$1',[lang]);await page.goto(origin+'/admin/settings/email');assert.equal(await page.locator('h1').innerText(),lang==='it'?'Email e notifiche':'Email and notifications');assert.equal(await page.locator('.sidebar-submenu [aria-current=page]').count(),1);
    const smtpSave=async(status=303)=>post('/admin/settings/email/transport','form[action$="/email/transport"] button[type=submit]',status);
    await page.locator('#mail-host').fill('smtp.example.test');await page.locator('#mail-port').fill('587');await page.locator('#mail-security').selectOption('starttls');await page.locator('#mail-username').fill('synthetic-user');await page.locator('#mail-password').fill('synthetic-browser-secret');await smtpSave();
    assert.equal(await page.locator('#mail-password').inputValue(),'');assert.ok(!(await page.content()).includes('synthetic-browser-secret'));
    assert.ok((await page.locator('#mail-password-state').innerText()).includes(lang==='it'?'Password salvata':'Password saved'));
    await smtpSave();assert.equal((await db.query('SELECT octet_length(password_ciphertext)>0 saved FROM email_transport')).rows[0].saved,true);
    await page.locator('#mail-host').fill('127.0.0.1');await page.locator('#mail-port').fill(String(smtpPort));await page.locator('#mail-security').selectOption('local');await page.locator('#mail-username').fill('');await page.locator('#mail-clear-password').check();await page.locator('#mail-smtp-enabled').check();await smtpSave();
    assert.equal((await db.query('SELECT octet_length(password_ciphertext) size FROM email_transport')).rows[0].size,0);
    await page.locator('#mail-host').focus();await page.keyboard.press('Tab');assert.equal(await page.locator('#mail-port').evaluate(e=>e===document.activeElement),true);
    await page.locator('#mail-from-name').fill('Talocart <Demo>');await page.locator('#mail-from-email').fill('sender@example.test');await page.locator('#mail-reply-to').fill('reply@example.test');await page.locator('#mail-admins').fill('admin@example.test\nADMIN@example.test');await page.locator('#mail-enabled').uncheck();await save(422);assert.equal(await page.locator('#mail-from-name').inputValue(),'Talocart <Demo>');assert.equal(await page.locator('#mail-error').count(),1);
    await page.locator('#mail-admins').fill('admin@example.test');await save();assert.ok(await page.locator('#mail-enabled').evaluate(e=>{const a=e.getBoundingClientRect(),b=e.closest('label').getBoundingClientRect();return Math.abs(a.top+a.height/2-b.top-b.height/2)<4;}),'Queue checkbox aligns with its label');await page.locator('#mail-from-name').focus();await page.keyboard.press('Tab');assert.equal(await page.locator('#mail-from-email').evaluate(e=>e===document.activeElement),true);assert.ok(await page.locator('#mail-from-email').evaluate(e=>getComputedStyle(e).outlineStyle!=='none'));
    await post('/admin/settings/email/check','form[action$="/email/check"] button');assert.ok((await page.locator('#mail-notice').innerText()).includes(lang==='it'?'Nessun messaggio':'No message'));
    // A silent SMTP peer must return to the settings page, beyond the ordinary HTTP deadline.
    const sockets=new Set();const silentSmtp=net.createServer(socket=>{sockets.add(socket);socket.on('error',()=>{});socket.on('close',()=>sockets.delete(socket));});
    try{
     await new Promise((resolve,reject)=>{silentSmtp.once('error',reject);silentSmtp.listen(0,'127.0.0.1',resolve);});
     await page.locator('#mail-port').fill(String(silentSmtp.address().port));await page.locator('#mail-timeout').fill('11');await smtpSave();
     const before=(await db.query('SELECT count(*) FROM email_outbox')).rows[0].count;
     await post('/admin/settings/email/check','form[action$="/email/check"] button');
     assert.equal(new URL(page.url()).searchParams.get('notice'),'connection-error');
     assert.ok((await page.locator('#mail-notice').innerText()).includes(lang==='it'?'Connessione SMTP non riuscita':'SMTP connection failed'));
     assert.equal((await db.query('SELECT count(*) FROM email_outbox')).rows[0].count,before);
     assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
     await page.screenshot({path:path.join(output,`${name}-${lang}-connection-timeout.png`),fullPage:true,animations:'disabled'});
    }finally{for(const socket of sockets)socket.destroy();await new Promise(resolve=>silentSmtp.close(resolve));}
    await page.locator('#mail-port').fill(String(smtpPort));await page.locator('#mail-timeout').fill('10');await smtpSave();
    for(const template of templates){
     const recipient=`preview-${tag}-${lang}-${template}@example.test`;
     await page.locator('#mail-recipient').fill(recipient);await page.locator('#mail-template').selectOption(template);await page.locator('#mail-locale').selectOption(lang);await post('/admin/settings/email/test','form[action$="/email/test"] button');
     const preview=await context.newPage();await preview.goto(origin+`/admin/settings/email/preview/${template}/${lang}`);assert.equal(await preview.locator('html').getAttribute('lang'),lang);assert.ok(await preview.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));assert.ok((await preview.locator('body').innerText()).includes(lang==='it'?'nessun ordine reale':'no real order'));if(template==='order_received')await preview.screenshot({path:path.join(output,`${name}-${lang}-template.png`),fullPage:true});await preview.close();
     const plain=await context.request.get(origin+`/admin/settings/email/preview/${template}/${lang}?plain=true`);assert.ok(plain.headers()['content-type'].startsWith('text/plain'));assert.ok((await plain.text()).includes('DEMO-0001'));
    }
    assert.equal((await db.query("SELECT count(*) FROM email_outbox WHERE state='pending' AND locale=$1",[lang])).rows[0].count,'6');
    // Persistent queue survives a real server restart while paused.
    await stop();cli('doctor');await start();await page.goto(origin+'/admin/settings/email');assert.equal(await page.locator('#mail-enabled').isChecked(),false);
    await page.locator('#mail-enabled').check();await save();
    for(let i=0;i<100;i++){if((await db.query("SELECT count(*) FROM email_outbox WHERE state='accepted' AND locale=$1",[lang])).rows[0].count==='6')break;await new Promise(r=>setTimeout(r,100));}
    assert.equal((await db.query("SELECT count(*) FROM email_outbox WHERE state='accepted' AND locale=$1",[lang])).rows[0].count,'6',JSON.stringify((await db.query('SELECT template,state,attempts,last_outcome FROM email_outbox')).rows));
    for(const template of templates){
     const recipient=`preview-${tag}-${lang}-${template}@example.test`;
     const found=await(await fetch(api+'/api/v1/search?query='+encodeURIComponent('to:'+recipient))).json();assert.equal(found.messages.length,1,'Exactly one captured test for each recipient');const captured=await(await fetch(api+'/api/v1/message/'+found.messages[0].ID)).json();
     assert.equal(captured.From.Address,'sender@example.test');assert.equal(captured.To[0].Address,recipient);assert.ok(captured.Text.includes('DEMO-0001'));assert.ok(captured.HTML.includes(`lang="${lang}"`));assert.ok(captured.Subject.includes(lang==='it'?'[PROVA]':'[TEST]'));assert.ok(captured.Text.includes(lang==='it'?'46,00 EUR':'46.00 EUR'));
     const headers=await(await fetch(api+'/api/v1/message/'+captured.ID+'/headers')).json();assert.ok(JSON.stringify(headers).includes('reply@example.test'));assert.ok(JSON.stringify(headers).includes('multipart/alternative'));assert.ok(JSON.stringify(headers).includes('@talocart.invalid'));
    }
    await page.goto(origin+'/admin/settings/email');assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));await page.screenshot({path:path.join(output,`${name}-${lang}-settings-log.png`),fullPage:true});
    const first=(await db.query('SELECT id FROM email_outbox ORDER BY id LIMIT 1')).rows[0].id;await page.goto(origin+`/admin/settings/email?attempts=${first}#mail-history`);assert.ok((await page.locator('#mail-history').innerText()).includes(lang==='it'?'Accettato':'Accepted'));
    // Simulate an exhausted synthetic job to exercise explicit retry in the actual form.
    await db.query("UPDATE email_outbox SET state='failed',accepted_at=NULL,last_outcome='transient' WHERE id=$1",[first]);await page.goto(origin+'/admin/settings/email');assert.ok(!(await page.locator('main').innerText()).includes(lang==='it'?'nuovo tentativo programmato':'retry scheduled'));assert.ok((await page.locator('main').innerText()).includes(lang==='it'?'Errore temporaneo':'Temporary failure'));await page.screenshot({path:path.join(output,`${name}-${lang}-failed-transient.png`),fullPage:true});await post(`/admin/settings/email/${first}/retry`,`form[action$="/${first}/retry"] button`);for(let i=0;i<100;i++){if((await db.query('SELECT state FROM email_outbox WHERE id=$1',[first])).rows[0].state==='accepted')break;await new Promise(r=>setTimeout(r,100));}assert.equal((await db.query('SELECT attempts FROM email_outbox WHERE id=$1',[first])).rows[0].attempts,lang==='it'?2:3);
    // Pause again for the next locale.
    await page.goto(origin+'/admin/settings/email');await page.locator('#mail-enabled').uncheck();await save();
   }
   assert.deepEqual(errors,[]);results.push({name,languages:['it','en'],javascript:js,captured:templates.length*2,passed:true});console.log(name,'passed');
  }finally{await stop();await context.close();await db.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);fs.rmSync(folder,{recursive:true,force:true});}
 }}finally{await browser.close();await db.end();}
 fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
})().catch(e=>{console.error(e);process.exitCode=1;});
