// Mandatory base customer groups: registration defaults and protected membership.
const assert = require('node:assert/strict'), fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { randomBytes } = require('node:crypto'), { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events'), { Client } = require('pg'), { chromium } = require('playwright');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test database required');
  const db = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL }); await db.connect();
  let browser;
  try { browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {}); } catch (error) { await db.end(); throw error; }
  const schema = 'talocart_customer_admin_' + process.pid + '_' + Date.now(), folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-customer-admin-'));
  const origin = 'http://127.0.0.1:3136', output = path.resolve('.local/verification/customer-admin/browser'); fs.mkdirSync(output, { recursive: true });
  const url = new URL(process.env.TALOCART_TEST_DATABASE_URL); url.searchParams.set('options', '-csearch_path=' + schema);
  const config = path.join(folder, 'talocart.toml'); fs.writeFileSync(config, '[server]\nlisten="127.0.0.1:3136"\n', { mode: 0o600 });
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
  const env = { ...process.env, TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: '127.0.0.1:3136', TALOCART_MEDIA_ROOT: path.join(folder, 'media'), TALOCART_LOG: 'warn' };
  let server; const results = [];
  const start = async () => {
    const fd = fs.openSync(path.join(output, 'server.log'), 'a', 0o600);
    server = spawn(binary, ['--config', config, 'serve'], { env, stdio: ['ignore', fd, fd] }); fs.closeSync(fd);
    for (let i = 0; i < 150; i++) { try { if ((await fetch(origin + '/health/ready')).ok) return; } catch {} await new Promise(r => setTimeout(r, 100)); }
    throw Error('Customer admin fixture readiness failed');
  };
  const stop = async () => { if (server && server.exitCode === null) { const done = once(server, 'exit'); server.kill('SIGTERM'); await done; } };
  try {
    await db.query('CREATE SCHEMA ' + schema); await db.query('SET search_path TO ' + schema);
    assert.equal(spawnSync(binary, ['--config', config, 'migrate'], { env, stdio: 'pipe' }).status, 0, 'Clean isolated migration'); await start();
    const setup = await browser.newContext(); const page = await setup.newPage(), password = randomBytes(20).toString('hex');
    await setup.addCookies([{ name: 'talocart_admin_language', value: 'it', url: origin }]); await page.goto(origin + '/setup');
    await page.getByLabel('Nome del negozio', { exact: true }).fill('Customer admin'); await page.getByLabel('Email amministratore').fill('owner@example.test'); await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByRole('button', { name: 'Crea negozio', exact: true }).click(); await page.waitForURL('**/admin/login');
    await page.getByLabel('Email amministratore').fill('owner@example.test'); await page.getByLabel('Password', { exact: true }).fill(password); await page.getByRole('button', { name: 'Accedi', exact: true }).click(); await page.waitForURL('**/admin');
    const cookies = (await setup.cookies()).filter(c => c.name === 'talocart_session');
    await page.goto(origin + '/admin/customer-groups/new');
    const csrf = await page.locator('main form [name=csrf]').first().inputValue();
    const languages = await setup.request.post(origin + '/admin/settings/storefront/languages', { data: new URLSearchParams({ csrf, primary_locale: 'en' }).toString() + '&locale=en&locale=it', headers: { 'content-type': 'application/x-www-form-urlencoded' }, maxRedirects: 0 }); assert.equal(languages.status(), 303);
    await setup.close();
    await db.query("INSERT INTO checkout_countries(country_code) VALUES('IT'),('FR')");
    const tax=(await db.query("INSERT INTO tax_rates(name,basis_points) VALUES('Zero',0) RETURNING id")).rows[0].id;
    const cls=(await db.query("INSERT INTO tax_classes(name,rate_id) VALUES('Zero',$1) RETURNING id",[tax])).rows[0].id;
    await db.query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Customer product','CUSTOMER-1','customer-product',1000,50,'published',$1)",[cls]);
    await db.query("UPDATE shipping_methods SET active=true WHERE id=2");await db.query("INSERT INTO shipping_method_countries(method_id,country_code) VALUES(2,'IT')");
    await db.query("UPDATE payment_methods SET active=true WHERE id=2");
    await db.query("UPDATE customer_field_policy SET overrides='{\"business.first_name\":\"optional\"}',revision=revision+1");
    const creationGroup=(await db.query("INSERT INTO customer_groups(name_it,name_en) VALUES('Clienti backend','Backend customers') RETURNING id")).rows[0].id;
    for (const [profile,width,js] of [['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]) for(const locale of ['en','it']) {
      const suffix=profile+'-'+locale;if(process.env.TALOCART_CUSTOMER_ADMIN_PROFILE&&!suffix.includes(process.env.TALOCART_CUSTOMER_ADMIN_PROFILE))continue;
      await db.query('UPDATE administrators SET locale=$1',[locale]);
      const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});await context.addCookies(cookies);
      const page=await context.newPage(),buyerContext=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js}),buyer=await buyerContext.newPage(),errors=[];
      page.on('pageerror',e=>errors.push(e.message));buyer.on('pageerror',e=>errors.push(e.message));
      const prefix=locale==='it'?'/it':'',email=suffix+'@example.test',buyerSecret='synthetic customer password';
      const submit=async(p,selector,expected=303)=>{const response=p.waitForResponse(r=>r.request().method()==='POST');const navigation=p.waitForNavigation({waitUntil:'load'});await p.locator(selector).click();assert.equal((await response).status(),expected);await navigation;};
      const section=async id=>{await page.locator('a[href="#customer-'+id+'"]').click();};
      const editBilling=async()=>{await section('billing');const details=page.locator('#customer-billing details');if(await details.getAttribute('open')===null){await details.locator('summary').focus();await page.keyboard.press('Enter');assert.notEqual(await details.getAttribute('open'),null);}};
      const shot=async(name)=>{assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));await page.screenshot({path:path.join(output,suffix+'-'+name+'.png'),fullPage:true});};
      try {
        await page.goto(origin+'/admin/customers');
        await shot('customer-list');
        const headerActions=await page.locator('.ui-page-header__actions .ui-button').evaluateAll(elements=>elements.map(e=>{const r=e.getBoundingClientRect();return {x:r.x,y:r.y,right:r.right,bottom:r.bottom};}));
        assert.equal(headerActions.length,2);
        assert.ok(headerActions[1].x>=headerActions[0].right+8||headerActions[1].y>=headerActions[0].bottom+8,'Customer list actions must have a visible gap');
        await require('./browser-customer-table.cjs')({page,db,origin,locale,js,output,suffix,group:creationGroup});
        const createLink=page.getByRole('link',{name:locale==='it'?'Nuovo cliente':'New customer',exact:true});
        await createLink.focus();await page.keyboard.press('Enter');await page.waitForURL('**/admin/customers/new');
        assert.equal(await page.locator('h1').innerText(),locale==='it'?'Nuovo cliente':'New customer');
        assert.equal(await page.locator('#ca-active').isChecked(),true);
        const defaultGroup=(await db.query('SELECT default_group_id FROM customer_group_configuration')).rows[0].default_group_id;
        assert.equal(await page.locator('#customer-group').inputValue(),String(defaultGroup));
        await page.locator('#ca-customer_type').selectOption('business');
        if(!js)await submit(page,'#customer-profile-form button[value=refresh]',200);
        assert.equal(await page.locator('#ca-first_name').getAttribute('required'),null);
        await page.locator('#ca-customer_type').selectOption('private');
        if(!js)await submit(page,'#customer-profile-form button[value=refresh]',200);
        assert.notEqual(await page.locator('#ca-first_name').getAttribute('required'),null);
        for(const [field,value] of Object.entries({first_name:'Alex',last_name:'Buyer',email,password:buyerSecret,password_confirmation:'different synthetic password'}))await page.locator('#ca-'+field).fill(value);
        await page.locator('#customer-group').selectOption(String(creationGroup));
        await submit(page,'#customer-profile-form button[value=save]',422);
        assert.equal(await page.locator('#ca-password_confirmation').getAttribute('aria-invalid'),'true');
        assert.equal(await page.locator('#ca-password').inputValue(),'');assert.equal(await page.locator('#ca-password_confirmation').inputValue(),'');
        assert.equal(await page.locator('#ca-email').inputValue(),email);
        assert.equal(await page.locator('#customer-group').inputValue(),String(creationGroup));
        await shot('new-customer-error');
        await page.locator('#ca-password').fill(buyerSecret);await page.locator('#ca-password_confirmation').fill(buyerSecret);
        const creationResponse=page.waitForResponse(r=>r.request().method()==='POST');
        const creationNavigation=page.waitForNavigation({waitUntil:'load'});
        await page.locator('#ca-email').press('Enter');assert.equal((await creationResponse).status(),303);await creationNavigation;
        assert.match(page.url(),/\/admin\/customers\/\d+\?saved=1/);
        assert.equal(String((await db.query('SELECT group_id FROM customers WHERE email=$1',[email])).rows[0].group_id),String(creationGroup));
        await buyer.goto(origin+prefix+'/account/login');
        await buyer.locator('#customer-email').fill(email);await buyer.locator('#customer-password').fill(buyerSecret);
        await submit(buyer,'#customer-form button[type=submit]:not([value=refresh])');
        await buyer.waitForURL(origin+prefix+'/account');
        const id=(await db.query('SELECT id FROM customers WHERE email=$1',[email])).rows[0].id,card=origin+'/admin/customers/'+id;
        await page.goto(card);assert.equal(await page.locator('#customer-profile-form').count(),1);assert.equal(await page.locator('#ca-email').inputValue(),email);
        if(js) {
          await page.locator('a[href="#customer-billing"]').click();
          const billingDetails=page.locator('#customer-billing details');
          if(await billingDetails.count())await billingDetails.locator('summary').click();
          await page.locator('#ca-billing_company').fill('Unsaved billing');
          await page.locator('a[href="#customer-profile"]').click();
          await page.locator('#ca-first_name').fill('Unsaved profile');
          page.once('dialog', d=>d.dismiss());
          await page.locator('#customer-profile-form button[value=save]').click();
          assert.equal(await page.locator('#ca-billing_company').inputValue(),'Unsaved billing','Saving another section must protect pending billing');
          assert.equal(await page.locator('#ca-first_name').inputValue(),'Unsaved profile');
          let warned=false;
          page.once('dialog',async d=>{warned=d.type()==='beforeunload';await d.dismiss();});
          await page.locator('main .ui-page-header__back').click();
          assert.equal(warned,true,'Leaving the page must warn about pending changes');
          assert.equal(await page.locator('#ca-first_name').inputValue(),'Unsaved profile');
          // Deliberately discard only the synthetic drafts before the remaining scenarios.
          page.once('dialog', d=>d.accept());await page.goto(card);
        }
        assert.equal(await page.locator('#customer-group-form').count(),0);
        assert.equal(await page.locator('#customer-profile-form #customer-group').count(),1);
        if(js){
          assert.equal(await page.locator('#customer-profile-form button[value=refresh]').isVisible(),false);
          await page.locator('#ca-customer_type').selectOption('business');
          assert.equal(await page.locator('#ca-first_name').getAttribute('required')!==null,await page.locator('[data-ca-field]:has(#ca-first_name)').getAttribute('data-business-required')==='true');
          await page.locator('#ca-customer_type').selectOption('private');
          assert.equal(await page.locator('#customer-billing').isVisible(),false);
        }else {
          assert.equal(await page.locator('#customer-profile-form button[value=refresh]').isVisible(),true);
          await page.locator('#ca-customer_type').selectOption('business');await submit(page,'#customer-profile-form button[value=refresh]',200);
          assert.equal(await page.locator('#ca-first_name').getAttribute('required'),null);
          await page.locator('#ca-customer_type').selectOption('private');await submit(page,'#customer-profile-form button[value=refresh]',200);
          assert.notEqual(await page.locator('#ca-first_name').getAttribute('required'),null);
        }
        await page.locator('#ca-first_name').focus();await page.keyboard.press('Tab');assert.equal(await page.locator(':focus').getAttribute('id'),'ca-last_name');
        assert.notEqual(await page.locator(':focus').evaluate(e=>getComputedStyle(e).outlineStyle),'none');
        await page.locator('#ca-phone').fill('+39 3331234567');await page.locator('#ca-first_name').fill('Edited');
        const stale=await context.newPage();await stale.goto(card);
        await submit(page,'#customer-profile-form button[value=save]');assert.equal((await db.query('SELECT first_name FROM customers WHERE id=$1',[id])).rows[0].first_name,'Edited');
        await stale.locator('#ca-first_name').fill('Stale draft');await submit(stale,'#customer-profile-form button[value=save]',409);assert.equal(await stale.locator('#ca-first_name').inputValue(),'Stale draft');await stale.close();
        // Invalid field and duplicate submissions must retain the entered profile.
        const bad=await context.request.post(card+'/profile',{form:{csrf:await page.locator('#customer-profile-form [name=csrf]').inputValue(),revision:await page.locator('#customer-profile-form [name=revision]').inputValue(),first_name:'<example>',last_name:'Buyer',email:'invalid',customer_type:'private',active:'true'},maxRedirects:0});assert.equal(bad.status(),422);assert.ok((await bad.text()).includes('ca-email-error'));
        await page.goto(card);
        const label=page.locator('label[for=ca-active]');assert.ok((await label.boundingBox()).width>120,'Checkbox labels must use the shared label component');
        assert.equal(await page.locator('#customer-profile-form fieldset').evaluate(e=>getComputedStyle(e).borderTopWidth),'0px','Native form grouping must not add a second frame');
        await shot('profile');
        await editBilling();
        const countryLabels=await page.locator('#ca-billing_country option:not([value=""])').allTextContents();
        assert.deepEqual(countryLabels,[...countryLabels].sort((a,b)=>a.toLowerCase()<b.toLowerCase()?-1:a.toLowerCase()>b.toLowerCase()?1:0));
        if(js)assert.equal(await page.locator('#customer-billing-form button[value=refresh]').isVisible(),false);
        // Billing is saved independently. Geographic requirements work with and without JS.
        await page.locator('#ca-billing_country').selectOption('IT');
        for(const [field,value] of Object.entries({billing_first_name:'Edited',billing_last_name:'Buyer',billing_company:'Example Srl',billing_street:'Billing Street',billing_number:'7',billing_postal:'00100',billing_city:'Roma',billing_region:'RM',vat_id:'12345678903',tax_id:'RSSMRA80A01H501U',recipient_code:'ABC1234',pec:'invoice@example.test'}))await page.locator('#ca-'+field).fill(value);
        await page.locator('#ca-invoice-requested').check();
        if(!js)await submit(page,'#customer-billing-form button[value=refresh]',200);
        await page.locator('#ca-billing_country').selectOption('FR');if(!js)await submit(page,'#customer-billing-form button[value=refresh]',200);
        assert.equal(await page.locator('#ca-billing_region').getAttribute('required'),null);
        await page.locator('#ca-billing_country').selectOption('IT');if(!js)await submit(page,'#customer-billing-form button[value=refresh]',200);
        assert.notEqual(await page.locator('#ca-billing_region').getAttribute('required'),null);
        await submit(page,'#customer-billing-form button[type=submit]:not([value=refresh])');assert.equal((await db.query('SELECT billing_company FROM customer_billing WHERE customer_id=$1',[id])).rows[0].billing_company,'Example Srl');await shot('billing');
        await section('profile');await page.locator('#ca-customer_type').selectOption('business');await submit(page,'#customer-profile-form button[value=save]');
        await editBilling();assert.equal(await page.locator('#ca-invoice-requested').count(),0);assert.equal(await page.locator('#customer-billing-form input[name=invoice_requested]').inputValue(),'true');
        await section('profile');await page.locator('#ca-customer_type').selectOption('private');await submit(page,'#customer-profile-form button[value=save]');
        await page.goto(card+'/addresses/new');assert.ok(await page.getByText(email,{exact:true}).isVisible(),'New address identifies its customer');await shot('address-new-context');for(const [field,value] of Object.entries({label:'Office',first_name:'Edited',last_name:'Buyer',company:'Example Srl',phone:'+39 3331234567',street:'Delivery Street',number:'1',postal:'00100',city:'Roma',region:'RM'}))await page.locator('#ca-'+field).fill(value);await page.locator('#ca-country').selectOption('IT');
        await submit(page,'#customer-address-form button[type=submit]:not([value=refresh])');
        const address=(await db.query('SELECT id FROM customer_addresses WHERE customer_id=$1',[id])).rows[0].id;
        await buyer.goto(origin+prefix+'/account/addresses');assert.ok((await buyer.locator('main').innerText()).includes('Delivery Street'));
        await buyer.goto(origin+prefix+'/products/customer-product');await buyer.getByRole('button',{name:locale==='it'?'Aggiungi al carrello':'Add to cart',exact:true}).click();await buyer.goto(origin+prefix+'/checkout');
        assert.equal(await buyer.locator('#billing_street').inputValue(),'Billing Street');assert.equal(await buyer.locator('#phone').inputValue(),'+39 3331234567');
        // Materialize a checkout draft, then changing master billing must leave it untouched.
        for(const [field,value] of Object.entries({recipient_first_name:'Edited',recipient_last_name:'Buyer',shipping_street:'Delivery Street',shipping_number:'1',shipping_postal:'00100',shipping_city:'Roma',shipping_region:'RM'}))await buyer.locator('#'+field).fill(value);
        await buyer.locator('#shipping_country').selectOption('IT');await submit(buyer,'#checkout-details button[value=continue]');await buyer.waitForURL(origin+prefix+'/checkout/methods');
        await page.goto(card);await editBilling();await page.locator('#ca-billing_street').fill('New master street');await submit(page,'#customer-billing-form button[type=submit]:not([value=refresh])');await buyer.goto(origin+prefix+'/checkout');assert.equal(await buyer.locator('#billing_street').inputValue(),'Billing Street');
        await page.goto(card+'/addresses/'+address);assert.ok(await page.getByText(email,{exact:true}).isVisible(),'Address editor identifies its customer');await shot('address-edit-context');await page.locator('#ca-street').fill('Updated Delivery Street');await page.locator('#ca-is_default').check();await submit(page,'#customer-address-form button[type=submit]:not([value=refresh])');assert.equal((await db.query('SELECT is_default FROM customer_addresses WHERE id=$1',[address])).rows[0].is_default,true);await page.goto(card+'/addresses/'+address);assert.equal(await page.locator('#ca-is_default').isChecked(),true);await page.goto(card);await section('addresses');assert.equal(await page.locator('[data-address-default]').count(),1);await shot('addresses');
        await buyer.goto(origin+prefix+'/account/addresses');assert.ok((await buyer.locator('main').innerText()).includes('Updated Delivery Street'));
        // Credential actions have native confirmation, one-response token visibility, and revocation.
        await page.goto(card);await section('access');await page.locator('#customer-access-form input[name=confirm]').check();await submit(page,'#customer-access-form button[value=recovery]',200);
        const recovery=await page.locator('#ca-recovery-token').inputValue();assert.equal(recovery.length,64);await page.goto(card);assert.equal(await page.locator('#ca-recovery-token').count(),0);
        await buyer.goto(origin+prefix+'/account');await buyer.waitForURL(/account\/login/);
        await buyer.goto(origin+prefix+'/account/reset');for(const [field,value] of Object.entries({token:recovery,password:'updated synthetic password',password_confirmation:'updated synthetic password'}))await buyer.locator('#customer-'+field).fill(value);await submit(buyer,'#customer-form > .checkout-actions button[type=submit]');
        await page.goto(card);await page.locator('#ca-active').uncheck();await submit(page,'#customer-profile-form button[value=save]');assert.equal(await page.locator('#customer-access-form button[value=recovery]').count(),0);await shot('inactive');
        await page.locator('#ca-active').check();await submit(page,'#customer-profile-form button[value=save]');
        // Changing email must leave address ownership intact.
        await page.locator('#ca-email').fill('changed-'+email);await submit(page,'#customer-profile-form button[value=save]');assert.equal((await db.query('SELECT customer_id FROM customer_addresses WHERE id=$1',[address])).rows[0].customer_id,id);
        await section('addresses');
        const del=page.locator('form[action$="/addresses/'+address+'/delete"]');await del.locator('[name=confirm]').check();await submit(page,'form[action$="/addresses/'+address+'/delete"] button');assert.equal((await db.query('SELECT count(*)::int n FROM customer_addresses WHERE id=$1',[address])).rows[0].n,0);
        assert.deepEqual(errors,[]);results.push({profile,locale,javascript:js,passed:true});console.log(suffix,'passed');
      }finally {await buyerContext.close();await context.close();}
    }
    await stop();await start();assert.equal((await fetch(origin+'/health/ready')).status,200);
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
  } finally {await stop();await browser.close();await db.query('DROP SCHEMA IF EXISTS '+schema+' CASCADE');await db.end();fs.rmSync(folder,{recursive:true,force:true});}
})().catch(e=>{console.error(e.stack);process.exitCode=1;});
