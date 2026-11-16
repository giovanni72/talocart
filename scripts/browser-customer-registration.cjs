// Isolated database/media and Mailpit only. Native registration and confirmation on both themes.
const assert = require('node:assert/strict'), fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const {randomBytes} = require('node:crypto'), {spawn, spawnSync} = require('node:child_process'), {once} = require('node:events');
const {Client} = require('pg'), {chromium} = require('playwright');
const net = require('node:net');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test database required');
  const mailpit = new URL(process.env.TALOCART_TEST_MAILPIT_URL || 'http://127.0.0.1:8025');
  assert.ok(['127.0.0.1','[::1]'].includes(mailpit.hostname), 'Loopback Mailpit required');
  const db = new Client({connectionString: process.env.TALOCART_TEST_DATABASE_URL}); await db.connect();
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? {channel: process.env.TALOCART_BROWSER_CHANNEL} : {});
  const schema = 'talocart_registration_' + process.pid + '_' + Date.now(), folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-registration-'));
  const port=await new Promise((resolve,reject)=>{const listener=net.createServer();listener.on('error',reject);listener.listen(0,'127.0.0.1',()=>{const port=listener.address().port;listener.close(()=>resolve(port));});});
  const origin = 'http://127.0.0.1:'+port, output = path.resolve('.local/verification/customer-registration/browser'); fs.mkdirSync(output, {recursive:true});
  const database = new URL(process.env.TALOCART_TEST_DATABASE_URL); database.searchParams.set('options', '-csearch_path=' + schema);
  const config = path.join(folder, 'talocart.toml'); fs.writeFileSync(config, '[server]\nlisten="127.0.0.1:'+port+'"\n', {mode:0o600});
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/debug/talocart');
  const env = {...process.env, TALOCART_DATABASE_URL:database.href, TALOCART_LISTEN:'127.0.0.1:'+port, TALOCART_MEDIA_ROOT:path.join(folder,'media'), TALOCART_LOG:'warn'};
  let server; const results = [];
  const stop = async () => {if(server && server.exitCode === null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
  const start = async () => {
    const fd = fs.openSync(path.join(output,'server.log'),'a',0o600); server=spawn(binary,['--config',config,'serve'],{env,stdio:['ignore',fd,fd]});fs.closeSync(fd);
    for(let i=0;i<150;i++){assert.equal(server.exitCode,null,'Synthetic server exited');try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Registration fixture readiness');
  };
  const post = async(page,selector,expected=303) => {
    const response=page.waitForResponse(r=>r.request().method()==='POST'),navigation=page.waitForNavigation({waitUntil:'load'});
    await page.locator(selector).click(); const [result]=await Promise.all([response,navigation]);assert.equal(result.status(),expected,'Unexpected POST status on '+new URL(result.url()).pathname);
  };
  const capture = async (address, minimum=1, previousId='') => {
    for(let i=0;i<150;i++){
      const found=await(await fetch(new URL('/api/v1/search?query='+encodeURIComponent('to:'+address),mailpit))).json();
      const message=found.messages.find(m=>m.ID!==previousId);
      if(found.messages.length>=minimum && message){return(await fetch(new URL('/api/v1/message/'+message.ID,mailpit))).json();}
      await new Promise(r=>setTimeout(r,100));
    }throw Error('Synthetic confirmation email not captured');
  };
  try {
    await db.query('CREATE SCHEMA '+schema); await db.query('SET search_path TO '+schema);
    assert.equal(spawnSync(binary,['--config',config,'migrate'],{env,stdio:'pipe'}).status,0,'Clean migration'); await start();
    const setup=await browser.newContext(),admin=await setup.newPage(),password=randomBytes(20).toString('hex');
    await setup.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);await admin.goto(origin+'/setup');
    await admin.getByLabel('Nome del negozio',{exact:true}).fill('Registration test');await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);
    await admin.getByRole('button',{name:'Crea negozio',exact:true}).click();await admin.waitForURL('**/admin/login');
    await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);await admin.getByRole('button',{name:'Accedi',exact:true}).click();await admin.waitForURL('**/admin');
    const cookies=(await setup.cookies()).filter(c=>c.name==='talocart_session');
    await admin.goto(origin+'/admin/settings/customer-fields');const csrf=await admin.locator('#customer-field-settings [name=csrf]').inputValue();
    const languages=await setup.request.post(origin+'/admin/settings/storefront/languages',{data:new URLSearchParams({csrf,primary_locale:'en'}).toString()+'&locale=en&locale=it',headers:{'content-type':'application/x-www-form-urlencoded'},maxRedirects:0});assert.equal(languages.status(),303);
    await db.query("UPDATE email_transport SET enabled=true,port=$1",[Number(process.env.TALOCART_TEST_SMTP_PORT || 1025)]);
    await db.query("UPDATE email_configuration SET enabled=true,from_name='Registration shop',from_email='sender@example.test',reply_to='support@example.test',admin_recipients=ARRAY['owner@example.test']");
    await setup.close();
    for(const theme of ['default','essential'])for(const [profile,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]])for(const locale of ['en','it']){
      const tag=[theme,profile,locale].join('-');
      if(process.env.TALOCART_REGISTRATION_PROFILE && !tag.includes(process.env.TALOCART_REGISTRATION_PROFILE))continue;
      // Independent synthetic profiles share loopback. Start each with a fresh
      // origin budget; application/session/address limits remain unchanged.
      // The HTTP origin-limit scenario separately verifies the real 429 boundary.
      await db.query("DELETE FROM auth_limits WHERE bucket LIKE 'customer-origin:reset:%'");
      await db.query('UPDATE storefront_settings SET theme_id=$1',[theme]);await db.query('UPDATE administrators SET locale=$1',[locale]);
      const staff=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});await staff.addCookies(cookies);const settings=await staff.newPage();
      const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js}),page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));settings.on('pageerror',e=>errors.push(e.message));
      const prefix=locale==='it'?'/it':'',url=s=>origin+prefix+s,secret='synthetic registration password',email='confirm-'+randomBytes(6).toString('hex')+'-'+tag+'@example.test';
      const shot=async(p,name)=>{assert.ok(await p.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),tag+' overflow');await p.screenshot({path:path.join(output,tag+'-'+name+'.png'),fullPage:true});};
      const mode=async value=>{await settings.goto(origin+'/admin/settings/customer-fields');await settings.locator('#registration-activation').selectOption(value);await post(settings,'#customer-registration-settings button');await settings.reload();assert.equal(await settings.locator('#registration-activation').inputValue(),value);};
      const register=async address=>{await page.goto(url('/account/register'));for(const[n,v]of Object.entries({first_name:'Alex',last_name:'Example',email:address,password:secret,password_confirmation:secret}))await page.locator('#customer-'+n).fill(v);await post(page,'#customer-form button:not([value=refresh])');};
      try {
        await mode('immediate');
        assert.equal(await settings.locator('#registration-guest-checkout').inputValue(),'true');
        await settings.locator('#registration-guest-checkout').selectOption('false');
        await post(settings,'#customer-registration-settings button');await settings.reload();
        assert.equal(await settings.locator('#registration-guest-checkout').inputValue(),'false');
        await settings.locator('#registration-guest-checkout').focus();await settings.keyboard.press('Tab');
        assert.equal(await settings.locator('#registration-activation').evaluate(e=>e===document.activeElement),true);
        await page.goto(url('/checkout'));await page.waitForURL(url('/account/login?next=%2Fcheckout'));
        assert.equal(await page.locator('#customer-checkout-login').count(),1);await shot(page,'checkout-account-required');await shot(settings,'guest-checkout');
        await settings.locator('#registration-guest-checkout').selectOption('true');
        await post(settings,'#customer-registration-settings button');await settings.reload();
        assert.equal(await settings.locator('#registration-guest-checkout').inputValue(),'true');
        await register('immediate-'+email);await page.waitForURL(url('/account'));
        assert.equal((await db.query('SELECT active FROM customers WHERE email=$1',['immediate-'+email])).rows[0].active,true);
        assert.equal((await db.query('SELECT count(*) FROM email_outbox WHERE recipient=$1',['immediate-'+email])).rows[0].count,'0');
        await post(page,'form[action$="/logout"] button');await mode('email');await shot(settings,'settings');
        await settings.locator('#customer-registration').screenshot({path:path.join(output,tag+'-registration-settings.png')});
        await settings.locator('#registration-activation').focus();await settings.keyboard.press('Tab');assert.equal(await settings.locator('#customer-registration-settings button').evaluate(e=>e===document.activeElement),true);
        await register(email);await page.waitForURL(url('/account/confirmation?changed=pending'));await shot(page,'pending');
        assert.equal((await db.query('SELECT active FROM customers WHERE email=$1',[email])).rows[0].active,false);
        await page.goto(url('/account/login'));await page.locator('#customer-email').fill(email);await page.locator('#customer-password').fill(secret);await post(page,'#customer-form button',401);
        const captured=await capture(email);assert.equal(captured.From.Address,'sender@example.test');assert.ok(captured.HTML.includes('lang="'+locale+'"'));assert.ok(captured.Text.includes('24'));
        const headers=await(await fetch(new URL('/api/v1/message/'+captured.ID+'/headers',mailpit))).json();assert.ok(JSON.stringify(headers).includes('multipart/alternative'));
        const oldLink=captured.Text.match(/http:\/\/127\.0\.0\.1:\d+\/\S*token=[a-f0-9]{64}/)[0];assert.equal(new URL(oldLink).origin,origin);assert.equal(new URL(oldLink).pathname,prefix+'/account/confirm');
        await db.query("UPDATE customer_email_confirmations SET expires_at=now()-interval '1 second',last_sent_at=now()-interval '2 minutes' WHERE email=$1",[email]);
        await page.goto(oldLink);await post(page,'#customer-confirmation-form button',422);await shot(page,'expired-link');
        await page.locator('a[href$="/account/confirmation"]').click();await page.locator('#customer-email').fill(email);await post(page,'#customer-form button');await page.waitForURL(url('/account/confirmation?changed=resent'));
        const replacement=await capture(email,2,captured.ID),link=replacement.Text.match(/http:\/\/127\.0\.0\.1:\d+\/\S*token=[a-f0-9]{64}/)[0];assert.ok(link!==oldLink,'Replacement link must differ');assert.equal(new URL(link).origin,origin);
        const response=await page.goto(link);assert.equal(response.headers()['referrer-policy'],'no-referrer');
        assert.equal((await db.query('SELECT active FROM customers WHERE email=$1',[email])).rows[0].active,false,'GET must not activate');
        await shot(page,'confirm');await page.locator('#customer-confirmation-form button').focus();assert.ok(await page.locator('#customer-confirmation-form button').evaluate(e=>e===document.activeElement));await page.keyboard.press('Enter');await page.waitForURL(url('/account/login?changed=confirmed'));
        assert.equal((await db.query('SELECT active FROM customers WHERE email=$1',[email])).rows[0].active,true);assert.equal(await page.locator('#customer-notice').count(),1);
        await page.goto(link);await post(page,'#customer-confirmation-form button',422);await shot(page,'used-link');
        await page.locator('a[href$="/account/confirmation"]').click();await page.locator('#customer-email').fill(email);await post(page,'#customer-form button');await page.waitForURL(url('/account/confirmation?changed=resent'));
        assert.equal((await db.query('SELECT count(*) FROM email_outbox WHERE recipient=$1',[email])).rows[0].count,'2','Active account gets no resend');
        await page.goto(url('/account/login'));await page.locator('#customer-email').fill(email);await page.locator('#customer-password').fill(secret);await post(page,'#customer-form button');await page.waitForURL(url('/account'));
        assert.ok((await page.locator('main').innerText()).includes(email));assert.deepEqual(errors,[]);results.push({theme,profile,locale,javascript:js,captured:true,passed:true});console.log(tag+' passed');
      } finally {await context.close();await staff.close();}
    }
    // Persistence across a real process restart on the same synthetic database.
    await stop();await start();assert.equal((await db.query('SELECT require_email_confirmation FROM customer_registration_settings')).rows[0].require_email_confirmation,true);
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
  } finally {await stop();await browser.close();await db.query('DROP SCHEMA IF EXISTS '+schema+' CASCADE');await db.end();fs.rmSync(folder,{recursive:true,force:true});}
})().catch(e=>{console.error(e.stack);process.exitCode=1;});
