// Dedicated schemas and synthetic accounts only. Native form policy across both themes.
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {randomBytes}=require('node:crypto'),{spawn,spawnSync}=require('node:child_process'),{once}=require('node:events');
const {Client}=require('pg'),{chromium}=require('playwright');
(async()=>{
 assert.ok(process.env.TALOCART_TEST_DATABASE_URL,'Dedicated test database required');
 const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 const schema='talocart_fields_'+process.pid+'_'+Date.now(),folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-fields-'));
 const origin='http://127.0.0.1:3135',output=path.resolve('.local/verification/customer-fields/browser');fs.mkdirSync(output,{recursive:true});
 const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options','-csearch_path='+schema);
 const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3135"\n',{mode:0o600});
 const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart');
 const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3135',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
 let server;const results=[];
 const start=async()=>{const fd=fs.openSync(path.join(output,'server.log'),'a',0o600);server=spawn(binary,['--config',config,'serve'],{env,stdio:['ignore',fd,fd]});fs.closeSync(fd);for(let i=0;i<150;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Readiness');};
 const stop=async()=>{if(server&&server.exitCode===null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
 try{
  await db.query('CREATE SCHEMA '+schema);await db.query('SET search_path TO '+schema);
  assert.equal(spawnSync(binary,['--config',config,'migrate'],{env,stdio:'pipe'}).status,0,'Clean migration');await start();
  const setup=await browser.newContext(),admin=await setup.newPage(),password=randomBytes(20).toString('hex');
  await setup.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);await admin.goto(origin+'/setup');
  await admin.getByLabel('Nome del negozio',{exact:true}).fill('Field policy');await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);
  await admin.getByRole('button',{name:'Crea negozio',exact:true}).click();await admin.waitForURL('**/admin/login');
  await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);await admin.getByRole('button',{name:'Accedi',exact:true}).click();await admin.waitForURL('**/admin');
  const cookies=(await setup.cookies()).filter(c=>c.name==='talocart_session');
  await admin.goto(origin+'/admin/settings/customer-fields');const csrf=await admin.locator('#customer-field-settings [name=csrf]').inputValue();
  const languages=await setup.request.post(origin+'/admin/settings/storefront/languages',{data:new URLSearchParams({csrf,primary_locale:'en'}).toString()+'&locale=en&locale=it',headers:{'content-type':'application/x-www-form-urlencoded'},maxRedirects:0});assert.equal(languages.status(),303);
  await db.query("INSERT INTO checkout_countries(country_code) VALUES('IT'),('FR')");
  const tax=(await db.query("INSERT INTO tax_rates(name,basis_points) VALUES('Zero',0) RETURNING id")).rows[0].id;
  const cls=(await db.query("INSERT INTO tax_classes(name,rate_id) VALUES('Zero',$1) RETURNING id",[tax])).rows[0].id;
  await db.query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Fields product','FIELDS-1','fields-product',1000,50,'published',$1)",[cls]);
  await db.query("UPDATE shipping_methods SET active=true WHERE id=2");await db.query("INSERT INTO shipping_method_countries(method_id,country_code) VALUES(2,'IT')");
  await db.query("UPDATE payment_methods SET active=true WHERE id=2");
  const submit=async(page,selector,expected=303)=>{const response=page.waitForResponse(r=>r.request().method()==='POST');const navigation=page.waitForNavigation({waitUntil:'load'});await page.locator(selector).click();assert.equal((await response).status(),expected);await navigation;};
  for(const theme of ['default','essential'])for(const locale of ['it','en'])for(const[profile,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]){
   const suffix=[theme,locale,profile].join('-');if(process.env.TALOCART_FIELDS_PROFILE&&!suffix.includes(process.env.TALOCART_FIELDS_PROFILE))continue;
   await db.query('UPDATE storefront_settings SET theme_id=$1',[theme]);await db.query('UPDATE administrators SET locale=$1',[locale]);
   const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});await context.addCookies(cookies);const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
   const buyerContext=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js}),buyer=await buyerContext.newPage();buyer.on('pageerror',e=>errors.push(e.message));
   const prefix=locale==='it'?'/it':'';
   try{
    await page.goto(origin+'/admin/settings');
    assert.equal(await page.locator('main a[href="/admin/settings/customer-fields"]').count(),0,'Removed standalone settings button');
    const settingsMenu=page.locator('.sidebar-group').filter({has:page.locator('a[href="/admin/settings/customer-fields"]')});
    const menuPaths=await settingsMenu.locator('.sidebar-submenu > a').evaluateAll(links=>links.map(link=>link.getAttribute('href')));
    assert.deepEqual(menuPaths.slice(0,3),['/admin/settings','/admin/settings/customer-fields','/admin/settings/payments']);
    const fieldsLink=settingsMenu.locator('a[href="/admin/settings/customer-fields"]');
    assert.equal(await fieldsLink.innerText(),locale==='it'?'Clienti e indirizzi':'Customers and addresses');
    await fieldsLink.focus();await fieldsLink.press('Enter');await page.waitForURL('**/admin/settings/customer-fields');
    assert.equal(await settingsMenu.getAttribute('open'),'');
    assert.equal(await fieldsLink.getAttribute('aria-current'),'page');
    assert.equal(await settingsMenu.locator('a[href="/admin/settings"]').getAttribute('aria-current'),null);
    await settingsMenu.screenshot({path:path.join(output,suffix+'-settings-menu.png')});
    assert.equal(await page.locator('h1').innerText(),locale==='it'?'Clienti e indirizzi':'Customers and addresses');
    const resetGaps=await page.locator('#customer-field-settings .editor-card').evaluateAll(cards=>cards.map(card=>({
     section:card.id,
     gap:card.querySelector('button[name=reset]').getBoundingClientRect().top-card.querySelector('.ui-field-matrix').getBoundingClientRect().bottom
    })));
    const helpGap=await page.locator('#customer-field-settings').evaluate(form=>{
     const actions=form.querySelector('.checkout-actions');
     return actions.nextElementSibling.getBoundingClientRect().top-actions.getBoundingClientRect().bottom;
    });
    assert.ok(helpGap>=12,`Reset help needs spacing below Save, got ${helpGap}px`);
    for(const item of resetGaps)assert.ok(item.gap>=12,`${item.section}: reset needs spacing above the last row, got ${item.gap}px`);
    for(const field of ['private-email','business-email','private-shipping_country','business-billing_country'])assert.equal(await page.locator('#cf-'+field).count(),0);
    for(const[key,mode]of Object.entries({'private-first_name':'hidden','private-last_name':'optional','private-shipping_company':'hidden','business-recipient_code':'required','business-pec':'required','business-delivery_notes':'required','business-shipping_street':'required'}))await page.locator('#cf-'+key).selectOption(mode);
    const stale=await context.newPage();await stale.goto(origin+'/admin/settings/customer-fields');
    await page.locator('#cf-business-pec').focus();assert.equal(await page.locator(':focus').getAttribute('id'),'cf-business-pec');await submit(page,'#customer-field-settings button[value=save]');
    await submit(stale,'#customer-field-settings button[value=save]',409);assert.equal(await stale.locator('#customer-field-settings button[value=save]').isDisabled(),true);await stale.close();
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));await page.screenshot({path:path.join(output,suffix+'-settings.png'),fullPage:true});await page.locator('#cf-billing').screenshot({path:path.join(output,suffix+'-billing-settings.png')});
    const beforeReset=(await db.query('SELECT revision FROM customer_field_policy')).rows[0].revision;
    await submit(page,'button[name=reset][value=billing]',200);
    assert.equal(await page.locator('#cf-business-pec').inputValue(),'optional');
    assert.equal(await page.locator('#cf-private-first_name').inputValue(),'hidden');
    assert.equal((await db.query('SELECT revision FROM customer_field_policy')).rows[0].revision,beforeReset,'Reset previews without persistence');
    await submit(page,'#customer-field-settings button[value=save]');
    const resetPolicy=(await db.query('SELECT overrides FROM customer_field_policy')).rows[0].overrides;
    assert.equal(resetPolicy['business.pec'],undefined);
    assert.equal(resetPolicy['private.first_name'],'hidden');
    for(const key of ['recipient_code','pec'])await page.locator('#cf-business-'+key).selectOption('required');
    await submit(page,'#customer-field-settings button[value=save]');
    await buyer.goto(origin+prefix+'/account/register');assert.equal(await buyer.locator('#customer-first_name').isVisible(),false);assert.equal(await buyer.locator('#customer-first_name').isDisabled(),true);
    for(const[field,value]of Object.entries({last_name:'Buyer',email:suffix+'@example.test',password:'synthetic customer password',password_confirmation:'synthetic customer password'}))await buyer.locator('#customer-'+field).fill(value);
    await submit(buyer,'#customer-form > .checkout-actions button[type=submit]:not([value=refresh])');await buyer.waitForURL(origin+prefix+'/account');
    assert.equal((await db.query('SELECT first_name FROM customers WHERE email=$1',[suffix+'@example.test'])).rows[0].first_name,'');
    await buyer.locator('#customer-type').selectOption('business');
    if(!js)await submit(buyer,'button[value=refresh]',200);
    assert.equal(await buyer.locator('#customer-first_name').isVisible(),true);await buyer.locator('#customer-first_name').fill('Business buyer');
    await submit(buyer,'#customer-form > .checkout-actions button[type=submit]:not([value=refresh])');assert.equal((await db.query('SELECT customer_type FROM customers WHERE email=$1',[suffix+'@example.test'])).rows[0].customer_type,'business');
    await buyer.goto(origin+prefix+'/products/fields-product');await buyer.getByRole('button',{name:locale==='it'?'Aggiungi al carrello':'Add to cart',exact:true}).click();await buyer.goto(origin+prefix+'/checkout');
    assert.equal(await buyer.locator('[name=customer_type][value=business]').isChecked(),true);
    assert.equal(await buyer.locator('#checkout-details [aria-invalid=true]').count(),0,'Profile prefill must not validate an unsubmitted checkout');
    assert.equal(await buyer.locator('#co-errors').count(),0);
    if(!js){
     await submit(buyer,'#checkout-details button[value=refresh]',200);
     assert.equal(await buyer.locator('#checkout-details [aria-invalid=true]').count(),0,'Refreshing requirements must not validate the incomplete form');
     assert.equal(await buyer.locator('#co-errors').count(),0);
    }
    await buyer.screenshot({path:path.join(output,suffix+'-checkout-initial.png'),fullPage:true});
    if(js){await buyer.locator('[name=customer_type][value=private]').check();await buyer.locator('#invoice_requested').check();assert.equal(await buyer.locator('[data-co-group=business]').isVisible(),false,'Default private billing must not show an empty business section');await submit(buyer,'#checkout-details button[value=continue]',422);await buyer.locator('[name=customer_type][value=business]').check();assert.equal(await buyer.locator('#billing_company').locator('..').locator('[data-required-mark]').isVisible(),true);}
    for(const[field,value]of Object.entries({email:suffix+'@example.test',first_name:'Business buyer',last_name:'Example',phone:'+39 3331234567',shipping_street:'Via Example',shipping_number:'1',shipping_postal:'00100',shipping_city:'Roma',shipping_region:'RM',billing_company:'Example company',vat_id:'12345678903'}))await buyer.locator('#'+field).fill(value);
    assert.equal(await buyer.locator('#invoice_channel').count(),0);
    if(js){await buyer.locator('#shipping_country').selectOption('FR');assert.equal(await buyer.locator('#shipping_region').getAttribute('required'),null);assert.equal(await buyer.locator('#shipping_region').locator('..').locator('[data-required-mark]').isVisible(),false);}
    await buyer.locator('#shipping_country').selectOption('IT');await buyer.locator('#billing_same').check();
    if(!js){const response=buyer.waitForResponse(r=>r.request().method()==='POST');const navigation=buyer.waitForNavigation({waitUntil:'load'});await buyer.locator('#email').press('Enter');assert.equal((await response).status(),422);await navigation;}else await submit(buyer,'#checkout-details button[value=continue]',422);
    for(const name of ['recipient_code','pec','delivery_notes'])assert.equal(await buyer.locator('#'+name).getAttribute('aria-invalid'),'true');
    for(const[field,value]of Object.entries({recipient_code:'ABC1234',pec:'invoice@example.test',delivery_notes:'Reception'}))await buyer.locator('#'+field).fill(value);
    await buyer.screenshot({path:path.join(output,suffix+'-billing.png'),fullPage:true});
    await submit(buyer,'#checkout-details button[value=continue]');await buyer.waitForURL(origin+prefix+'/checkout/methods');
    const saved=(await db.query('SELECT invoice_channel,recipient_code,pec FROM checkout_details WHERE email=$1',[suffix+'@example.test'])).rows[0];assert.deepEqual(saved,{invoice_channel:'both',recipient_code:'ABC1234',pec:'invoice@example.test'});
    await page.goto(origin+'/admin/settings/customer-fields');await page.locator('#cf-business-shipping_street').selectOption('hidden');await submit(page,'#customer-field-settings button[value=save]');
    await buyer.goto(origin+prefix+'/checkout');assert.equal(await buyer.locator('#billing_same').isChecked(),false);assert.equal(await buyer.locator('#shipping_street').isVisible(),false);assert.equal(await buyer.locator('#billing_street').isVisible(),true);assert.equal(await buyer.locator('#billing_street').getAttribute('aria-invalid'),'true');await buyer.locator('#billing_street').fill('Independent billing street');await submit(buyer,'#checkout-details button[value=continue]');
    assert.ok(await buyer.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));assert.deepEqual(errors,[]);results.push({theme,locale,profile,javascript:js,passed:true});console.log(suffix,'passed');
   }finally{await buyerContext.close();await context.close();}
  }
  await setup.close();await stop();await start();assert.equal((await fetch(origin+'/health/ready')).status,200);
  fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
 }finally{await stop();await browser.close();await db.query('DROP SCHEMA IF EXISTS '+schema+' CASCADE');await db.end();fs.rmSync(folder,{recursive:true,force:true});}
})().catch(e=>{console.error(e.stack);process.exitCode=1;});
