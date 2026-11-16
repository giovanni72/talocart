// Administrative drafts and revisions against disposable synthetic resources.
const assert=require('node:assert/strict');
const fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {spawn,spawnSync}=require('node:child_process'),{once}=require('node:events'),{randomBytes}=require('node:crypto');
const {Client}=require('pg'),{chromium}=require('playwright');
const presentation=require('./theme-presentation-helper.cjs');
(async()=>{
 const raw=process.env.TALOCART_TEST_DATABASE_URL;assert.ok(raw,'Dedicated test database required');
 const parsed=new URL(raw);assert.ok(parsed.hostname==='127.0.0.1'&&parsed.pathname.endsWith('_test'));
 const db=new Client({connectionString:raw});await db.connect();
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 const output=path.resolve('.local/verification/order-editor');fs.mkdirSync(output,{recursive:true});const results=[];
 try {for(const[name,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]].filter(([n])=>!process.env.TALOCART_ORDER_TEST_PROFILE||n===process.env.TALOCART_ORDER_TEST_PROFILE)){
  const baseline=results.length;
  const schema=`talocart_order_editor_${process.pid}_${Date.now()}`,folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-editor-')),origin='http://127.0.0.1:3129';
  const url=new URL(raw);url.searchParams.set('options',`-csearch_path=${schema}`);
  const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3129"\n',{mode:0o600});
  const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3129',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
  const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/debug/talocart');let server;
  const query=(sql,args)=>db.query(sql,args);
  const cli=command=>{const r=spawnSync(binary,['--config',config,command],{env,encoding:'utf8'});assert.equal(r.status,0,`CLI ${command} failed (private diagnostics omitted)`);};
  const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
  try {
   await query(`CREATE SCHEMA ${schema}`);await query(`SET search_path TO ${schema}`);cli('migrate');
   const fd=fs.openSync(path.join(output,`${name}-server.log`),'a',0o600);server=spawn(binary,['--config',config,'serve'],{env,stdio:['ignore',fd,fd]});fs.closeSync(fd);
   let ready=false;for(let i=0;i<100;i++){try{if((await fetch(origin+'/health/ready')).ok){ready=true;break;}}catch{}await new Promise(r=>setTimeout(r,100));}assert.ok(ready,'Readiness failed');
   await context.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);
   const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));const password=randomBytes(20).toString('hex');
   await page.goto(origin+'/setup');await page.getByLabel('Nome del negozio',{exact:true}).fill('Synthetic order editor');await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);await page.getByRole('button',{name:'Crea negozio',exact:true}).click();await page.waitForURL('**/admin/login');await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(password);await page.getByRole('button',{name:'Accedi',exact:true}).click();await page.waitForURL('**/admin');
   await page.goto(origin+'/admin/components');assert.equal(await page.locator('main').count(),1);
   await query("INSERT INTO checkout_countries(country_code) VALUES('IT')");const tax=(await query("INSERT INTO tax_rates(name,basis_points) VALUES('Synthetic VAT',2200) RETURNING id")).rows[0].id;const cls=(await query("INSERT INTO tax_classes(name,rate_id) VALUES('Synthetic VAT',$1) RETURNING id",[tax])).rows[0].id;
   const product=(await query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Phone order product','PHONE-ORDER','phone-order-product',1000,1000,'published',$1) RETURNING id",[cls])).rows[0].id;
   await query("UPDATE shipping_methods SET active=true,fee_value=200,tax_class_id=$1 WHERE id=1",[cls]);await query("INSERT INTO shipping_method_countries(method_id,country_code) VALUES(1,'IT')");
   await query("UPDATE payment_methods SET active=true,fee_kind='fixed',fee_value=50,tax_class_id=$1,beneficiary=CASE WHEN id=1 THEN 'Synthetic bank' ELSE '' END,iban=CASE WHEN id=1 THEN 'IT60X0542811101000000123456' ELSE '' END WHERE code IN ('bank_transfer','cash_on_delivery')",[cls]);
   const customer=(await query("INSERT INTO customers(group_id,email,password_hash,first_name,last_name,phone) VALUES(1,'saved@example.test','synthetic-no-login','Saved','Buyer','+393331234567') RETURNING id")).rows[0].id;
   const submit=async(action,expected=303)=>{const [response]=await Promise.all([page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname.includes('/admin/orders')),page.waitForNavigation({waitUntil:'load'}),action()]);assert.equal(response.status(),expected);};
   const reveal=async(selector)=>{if(!js)return page.locator(selector);const tab=await page.locator(selector).evaluate(el=>{const panel=el.closest('[role=tabpanel]');return panel&&panel.hidden?panel.getAttribute('aria-labelledby'):'';});if(tab)await page.locator('#'+tab).click();return page.locator(selector);};
   const checkpoint=async(label)=>{assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),`${label}: no horizontal overflow`);await presentation.verify(page,label);await presentation.screenshot(page,{path:path.join(output,`${label}-${name}.png`),fullPage:true});};
   for(const theme of ['default','essential'])for(const lang of ['it','en']){
    await query('UPDATE storefront_settings SET theme_id=$1',[theme]);await query('UPDATE administrators SET locale=$1',[lang]);await query('UPDATE tax_rates SET basis_points=$1 WHERE id=$2',[lang==='it'?507:750,tax]);await query('UPDATE catalog_products SET price_cents=1000 WHERE id=$1',[product]);
    await page.goto(origin+'/admin/orders');await submit(()=>page.locator('form[action="/admin/orders/new"] button').click());await page.waitForURL('**/admin/orders/drafts/*');const draftUrl=page.url();
    const bad=await context.request.post(draftUrl,{form:{csrf:'bad',revision:'0',line_count:'0',action:'save'},maxRedirects:0});assert.equal(bad.status(),403);
    if(lang==='en') {await (await reveal('#oe-customer-search')).fill('saved@example.test');await submit(()=>page.locator('button[value="search-customers"]').click());await submit(()=>page.locator(`button[value="customer:${customer}"]`).click());assert.equal(await page.locator('#oe-first_name').inputValue(),'Saved');}
    for(const[k,v]of Object.entries({email:`editor-${name}-${theme}-${lang}@example.test`,first_name:'Alex',last_name:'Example',phone:'+393331234567',shipping_street:'Via Example',shipping_number:'1',shipping_postal:'00100',shipping_city:'Roma',shipping_region:'RM'}))await (await reveal('#oe-'+k)).fill(v);
    await (await reveal('#oe-shipping_country')).selectOption('IT');await (await reveal('#oe-locale')).selectOption(lang);
    await (await reveal('#oe-product-search')).fill('PHONE-ORDER');await submit(()=>page.locator('button[value="search-products"]').click());await submit(()=>page.locator(`button[value="add:${product}:0"]`).click());
    await page.locator('#oe-quantity-0').fill('2');await page.locator('#oe-price-0').fill('8.00');await (await reveal('#oe-shipping')).selectOption('1');await (await reveal('#oe-payment')).selectOption('2');await page.locator('#oe-reason').fill(lang==='en'?'Phone <order>\nSecond line':'Phone <order>');
    await (await reveal('#oe-price-0')).focus();await page.keyboard.press('Tab');assert.ok(await page.locator('button[value="remove:0"]').evaluate(el=>el===document.activeElement));
    await submit(()=>page.locator('button[value="save"]').click());assert.equal((await query('SELECT count(*) FROM orders')).rows[0].count,String(results.length-baseline));assert.equal((await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock,1000);
    const extraDrafts=(await query("INSERT INTO order_drafts(payload,administrator_id) SELECT '{}',a.id FROM administrators a CROSS JOIN generate_series(1,50) WHERE a.id=(SELECT min(id) FROM administrators) RETURNING id")).rows.map(r=>r.id);
    await page.goto(origin+'/admin/orders/drafts');assert.equal(await page.locator(`a[href="${new URL(draftUrl).pathname}"]`).count(),0);await page.locator('.shared-pagination a[rel=next]').click();await page.waitForURL('**/admin/orders/drafts?page=2');await checkpoint(`${theme}-${lang}-drafts-page-two`);await page.locator(`a[href="${new URL(draftUrl).pathname}"]`).click();
    await query('DELETE FROM order_drafts WHERE id=ANY($1::bigint[])',[extraDrafts]);assert.equal(await page.locator('#oe-price-0').inputValue(),'8.00');await checkpoint(`${theme}-${lang}-editor`);
    await submit(()=>page.locator('button[value="preview"]').click());assert.equal(await page.locator('#order-editor-form').count(),0);assert.equal(await page.locator('#order-publish-form').count(),1);assert.equal(await page.locator('.order-editor .ui-table tbody tr td:last-child small').first().innerText(),lang==='it'?'IVA: 5,07%':'VAT: 7.50%');assert.ok((await page.locator('#oe-preview-total').innerText()).includes(lang==='it'?'18,50':'18.50'));await checkpoint(`${theme}-${lang}-preview`);
    await query('UPDATE catalog_products SET price_cents=1100 WHERE id=$1',[product]);await page.locator('#order-publish-form [name=confirm]').check();await submit(()=>page.locator('#order-publish-form button').click(),409);assert.equal((await query('SELECT count(*) FROM orders')).rows[0].count,String(results.length-baseline));assert.equal(await page.locator('#order-editor-error').count(),1);
    await submit(()=>page.locator('button[value="preview"]').click());await page.locator('#order-publish-form [name=confirm]').check();const replay=await page.locator('#order-publish-form').evaluate(form=>Object.fromEntries(new FormData(form)));await submit(()=>page.locator('#order-publish-form button').click());await page.waitForURL(/\/admin\/orders\/\d+$/);const detailUrl=page.url(),id=new URL(detailUrl).pathname.split('/').pop();
    if(lang==='en') assert.equal((await query('SELECT customer_id FROM customer_orders WHERE order_id=$1',[id])).rows[0].customer_id,customer);
    const again=await context.request.post(draftUrl+'/confirm',{form:replay,maxRedirects:0});assert.equal(again.status(),303);assert.equal((await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock,998);
    await submit(()=>page.locator(`form[action="/admin/orders/${id}/edit"] button`).click());assert.equal(await page.locator('#oe-price-0').inputValue(),'8,00');await (await reveal('#oe-quantity-0')).fill('3');await (await reveal('#oe-shipping_street')).fill('Via Revised');await page.locator('#oe-reason').fill('Customer requested three');await submit(()=>page.locator('button[value="preview"]').click());await page.locator('#order-publish-form [name=confirm]').check();await submit(()=>page.locator('#order-publish-form button').click());
    assert.equal(page.url(),detailUrl);assert.equal((await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock,997);assert.equal(await page.locator('#order-lines tbody tr').count(),1);assert.ok((await page.locator('#order-lines').innerText()).includes('3'));assert.equal(await page.locator('#order-edit-history details').count(),2);assert.match(await page.locator('#order-edit-history summary').first().innerText(),/#\d+/);await page.locator('#order-edit-history details').first().locator('summary').click();assert.ok((await page.locator('#order-edit-history').innerText()).includes('Via Revised'));await checkpoint(`${theme}-${lang}-revised`);
    const cancel=page.locator(`form[action="/admin/orders/${id}/cancel"]`);await cancel.locator('[name=confirm]').check();await submit(()=>cancel.locator('button').click());assert.equal((await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock,1000);assert.equal(await page.locator(`form[action="/admin/orders/${id}/edit"]`).count(),0);
    results.push({profile:name,theme,lang,js,created:true,revised:true,stockRestored:true});
   }
   assert.deepEqual(errors,[]);
  } finally {
   await context.close();if(server&&server.exitCode===null){const ended=once(server,'exit');server.kill('SIGTERM');await ended;}
   await query('SET search_path TO public');await query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);fs.rmSync(folder,{recursive:true,force:true});
  }
 }}finally{await browser.close();await db.end();}
 fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2)+'\n');console.log(`Order editor: ${results.length} theme/language/profile cases passed`);
})().catch(error=>{console.error(error);process.exitCode=1;});
