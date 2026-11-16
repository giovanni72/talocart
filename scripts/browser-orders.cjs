const presentation = require('./theme-presentation-helper.cjs');
// Real two-step order flow, isolated data and local Mailpit. Never deletes captured messages.
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {randomBytes}=require('node:crypto'),{spawn,spawnSync}=require('node:child_process'),{once}=require('node:events'),{Client}=require('pg'),{chromium}=require('playwright');
(async()=>{
 assert.ok(process.env.TALOCART_TEST_DATABASE_URL,'Dedicated database required');
 const api=process.env.TALOCART_TEST_MAILPIT_URL||'http://127.0.0.1:8025';assert.equal(new URL(api).hostname,'127.0.0.1');assert.ok((await fetch(api+'/api/v1/info')).ok,'Local Mailpit required');
 const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});const output=path.resolve('.local/verification/orders');fs.mkdirSync(output,{recursive:true});const results=[];
 try {for(const[name,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]].filter(([n])=>!process.env.TALOCART_ORDER_TEST_PROFILE||n===process.env.TALOCART_ORDER_TEST_PROFILE)){
  const schema=`talocart_orders_${process.pid}_${Date.now()}`,folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-orders-')),origin='http://127.0.0.1:3116',tag=randomBytes(6).toString('hex');const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options',`-csearch_path=${schema}`);const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3116"\n',{mode:0o600});
  const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3116',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart');let server;
  const cli=command=>{let r=spawnSync(binary,['--config',config,command],{env,encoding:'utf8'});assert.equal(r.status,0,`CLI ${command} failed (private diagnostics omitted)`);};
  const start=async()=>{const fd=fs.openSync(path.join(output,`${name}-server.log`),'a',0o600);server=spawn(binary,['--config',config,'serve'],{env,stdio:['ignore',fd,fd]});fs.closeSync(fd);for(let i=0;i<100;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Readiness failed');};
  const stop=async()=>{if(server&&server.exitCode===null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
  const query=(sql,args)=>db.query(sql,args),adminContext=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
  try {
   await query(`CREATE SCHEMA ${schema}`);await query(`SET search_path TO ${schema}`);cli('migrate');await start();const admin=await adminContext.newPage(),errors=[];admin.on('pageerror',e=>errors.push(e.message));await adminContext.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);
   const password=randomBytes(20).toString('hex');await admin.goto(origin+'/setup');await admin.getByLabel('Nome del negozio',{exact:true}).fill('Order test');await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);await admin.getByRole('button',{name:'Crea negozio',exact:true}).click();await admin.waitForURL('**/admin/login');await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);await admin.getByRole('button',{name:'Accedi',exact:true}).click();await admin.waitForURL('**/admin');
   for (const locale of ['it','en']) {
    await query('UPDATE administrators SET locale=$1',[locale]); await admin.goto(origin+'/admin/orders');
    assert.equal(await admin.locator('.order-table thead th[scope=col]').count(),7);
    assert.equal(await admin.locator('.order-table tbody td[colspan="7"] .ui-empty').count(),1);
    assert.equal(await admin.locator('.shared-pagination [aria-disabled=true]').count(),2);
    assert.ok((await admin.locator('.order-table').innerText()).includes(locale==='it'?'Nessun ordine':'No orders'));
   }
    await db.query("UPDATE email_transport SET enabled=true,port=$1",[Number(process.env.TALOCART_TEST_SMTP_PORT || 1025)]);
   await query("INSERT INTO checkout_countries(country_code) VALUES('IT')");const rate=(await query("INSERT INTO tax_rates(name,basis_points) VALUES('Order VAT',2200) RETURNING id")).rows[0].id;const tax=(await query("INSERT INTO tax_classes(name,rate_id) VALUES('Order VAT',$1) RETURNING id",[rate])).rows[0].id;const product=(await query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Order product','ORDER-TEST','order-product',1000,10,'published',$1) RETURNING id",[tax])).rows[0].id;
   await query("UPDATE shipping_methods SET active=true,fee_value=200,tax_class_id=$1 WHERE id=1",[tax]);await query("INSERT INTO shipping_method_countries(method_id,country_code) VALUES(1,'IT')");await query("UPDATE payment_methods SET active=true,fee_kind='fixed',fee_value=50,tax_class_id=$1,beneficiary=CASE WHEN id=1 THEN 'Example bank' ELSE '' END,iban=CASE WHEN id=1 THEN 'IT60X0542811101000000123456' ELSE '' END",[tax]);
   const primary=(await query('SELECT primary_locale FROM storefront_settings')).rows[0].primary_locale;
   for(const lang of ['it','en']){
    const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js}),page=await context.newPage();page.on('pageerror',e=>errors.push(e.message));const prefix=lang===primary?'':`/${lang}`,route=prefix+'/checkout',recipient=`order-${tag}-${lang}@example.test`,shop=`shop-${tag}-${lang}@example.test`;
    try{
     await query('UPDATE administrators SET locale=$1',[lang]);await admin.goto(origin+'/admin/settings#date-time');
     const regional=admin.locator('#settings-editor'),zone=lang==='it'?'Europe/Rome':'America/New_York',style=lang==='it'?'dmy':'mdy';
     const saveRegional=async()=>{const done=admin.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname==='/admin/settings/save');await regional.getByRole('button',{name:lang==='it'?'Salva impostazioni':'Save settings',exact:true}).click();assert.equal((await done).status(),303);await admin.waitForLoadState('load');};
     assert.equal(await admin.locator('#platform-time-zone').inputValue(),'Europe/Rome');
     await admin.locator('#platform-time-zone').selectOption('UTC');await admin.locator('#platform-date-format').selectOption('ymd');await saveRegional();assert.match(await admin.locator('#date-time output').innerText(),/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
     await admin.locator('#platform-time-zone').selectOption(zone);await admin.locator('#platform-date-format').selectOption(style);const staleRegional=await regional.evaluate(form=>({csrf:form.elements.csrf.value,time_zone:form.elements.time_zone.value,date_format:form.elements.date_format.value,revision:form.elements.regional_revision.value}));await saveRegional();
     const preview=await admin.locator('#date-time output').innerText();assert.ok(!preview.includes(zone));assert.match(preview,/^\d{2}\/\d{2}\/\d{4} \d{2}:\d{2}:\d{2}$/);assert.equal(await admin.locator('#platform-time-zone').inputValue(),zone);assert.equal(await admin.locator('#platform-date-format').inputValue(),style);
     const conflict=await adminContext.request.post(origin+'/admin/settings/date-time',{form:staleRegional,maxRedirects:0});assert.equal(conflict.status(),409);
     const invalid=await regional.evaluate(form=>({csrf:form.elements.csrf.value,time_zone:form.elements.time_zone.value,date_format:form.elements.date_format.value,revision:form.elements.regional_revision.value}));invalid.time_zone='Invalid/Zone';const bad=await adminContext.request.post(origin+'/admin/settings/date-time',{form:invalid,maxRedirects:0});assert.equal(bad.status(),422);assert.ok((await bad.text()).includes('id="regional-error"'));
     await admin.locator('#platform-time-zone').focus();await admin.keyboard.press('Tab');assert.equal(await admin.locator('#platform-date-format').evaluate(el=>el===document.activeElement),true);assert.ok(await admin.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));{ const shot = {path:path.join(output,`${name}-${lang}-date-time.png`),fullPage:true}; await presentation.verify(admin, path.basename(shot.path)); await presentation.screenshot(admin, shot); }
     await query("UPDATE email_configuration SET enabled=false,from_name='Synthetic shop',from_email='sender@example.test',reply_to='support@example.test',admin_recipients=ARRAY[$1::text]",[shop]);await query("UPDATE shipping_methods SET fee_value=200 WHERE id=1");
     await page.goto(origin+prefix+'/products/order-product');await page.getByRole('button',{name:lang==='it'?'Aggiungi al carrello':'Add to cart',exact:true}).click();await page.goto(origin+route);
     for(const[k,v]of Object.entries({email:recipient,first_name:'Alex',last_name:'Example',phone:'+393331234567',shipping_street:'Via Example',shipping_number:'1',shipping_postal:'00100',shipping_city:'Roma',shipping_region:'RM'}))await page.locator('#'+k).fill(v);await page.locator('#shipping_country').selectOption('IT');
     const submit=async(action,status)=>{
       if(action==='preview'&&new URL(page.url()).pathname===route){const done=page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname===route);await page.locator('button[value=continue]').click();assert.equal((await done).status(),303);await page.waitForURL(origin+route+'/methods');return;}
       if(action==='preview'&&js){await page.waitForFunction(()=>!document.querySelector('#checkout-methods-region[aria-busy=true]'));return;}
       const [response]=await Promise.all([page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname===route+'/methods'),page.waitForNavigation({waitUntil:'load'}),page.locator(`button[value=${action==='preview'?'update':action}]`).click()]);assert.equal(response.status(),action==='preview'?303:status);
     };
     await submit('preview',200);await page.locator('[name=shipping_method_id][value="1"]').check();await submit('preview',200);
     // Native keyboard radio navigation also works with JavaScript disabled.
     const payment=lang==='it'?'1':'2',other=payment==='1'?'2':'1';await page.locator(`[name=payment_method_id][value="${other}"]`).check();await submit('preview',200);await page.locator(`[name=payment_method_id][value="${other}"]`).focus();await page.keyboard.press(payment==='1'?'ArrowUp':'ArrowDown');assert.equal(await page.locator(`[name=payment_method_id][value="${payment}"]`).isChecked(),true);await submit('preview',200);assert.match(await page.locator('#checkout-summary').innerText(),/12[,.]50/);
     await query("UPDATE shipping_methods SET fee_value=300 WHERE id=1");await submit('confirm',409);assert.equal((await query('SELECT count(*) FROM orders')).rows[0].count,lang==='it'?'0':'1');assert.equal(await page.locator('button[value=confirm]').isEnabled(),true);assert.match(await page.locator('#checkout-summary').innerText(),/13[,.]50/);
     await query('UPDATE administrators SET locale=$1',[lang]);await admin.goto(origin+`/admin/catalog/products/${product}`);await admin.locator(js?'#product-description-visual':'#product-description').fill('Edited while customer checks out');
     const replay=await page.locator('#checkout-methods').evaluate(form=>Object.fromEntries(new FormData(form)));replay.action='confirm';await submit('confirm',303);await page.waitForURL('**/checkout/orders/ORD-*');const receiptUrl=page.url(),reference=new URL(receiptUrl).pathname.split('/').pop();const order=(await query("SELECT id,source_cart_id,extract(year FROM created_at AT TIME ZONE r.time_zone)::integer AS year FROM orders CROSS JOIN platform_regional_settings r WHERE r.singleton AND reference=$1",[reference])).rows[0];const annualCount=(await query('SELECT count(*)::integer AS total FROM orders WHERE reference LIKE $1',[`ORD-${order.year}-%`])).rows[0].total;assert.equal(reference,`ORD-${order.year}-${annualCount}`);
     assert.equal((await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock,9);assert.equal((await query('SELECT count(*) FROM guest_cart_items WHERE cart_id=$1',[order.source_cart_id])).rows[0].count,'0');assert.equal((await query('SELECT count(*) FROM email_outbox WHERE event_key LIKE $1',[`order:${order.id}:%`])).rows[0].count,'2');
     const staleSave=async()=>{const response=admin.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname===`/admin/catalog/products/${product}`);await admin.locator('button[form="product-editor-form"]').click();assert.equal((await response).status(),409);await admin.waitForLoadState('load');assert.ok((await admin.locator('#product-error').innerText()).includes(lang==='it'?'La disponibilità è cambiata':'Stock availability has changed'));};
     await staleSave();assert.equal((await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock,9);{ const shot = {path:path.join(output,`${name}-${lang}-stale-stock.png`),fullPage:true}; await presentation.verify(admin, path.basename(shot.path)); await presentation.screenshot(admin, shot); }
     await admin.goto(origin+`/admin/catalog/products/${product}`);const beforeRestore=await admin.locator('#product-editor-form').evaluate(form=>Object.fromEntries(new FormData(form)));
     const repeated=await context.request.post(origin+route+'/methods',{form:replay,maxRedirects:0});assert.equal(repeated.status(),303);assert.equal(new URL(repeated.headers().location,origin).href,receiptUrl);assert.equal((await query('SELECT count(*) FROM order_stock_moves')).rows[0].count,lang==='it'?'1':'3');
     const alien=await browser.newContext();assert.equal((await alien.request.get(receiptUrl)).status(),403);await alien.close();
     await stop();cli('doctor');await start();const response=await page.goto(receiptUrl);assert.equal(response.headers()['cache-control'],'no-store');assert.ok((await page.locator('main').innerText()).includes(reference));assert.ok(!(await page.locator('.order-detail').innerText()).includes(zone));assert.ok((await page.locator('.order-detail').innerText()).includes(lang==='it'?'13,50 EUR':'13.50 EUR'),'Localized receipt total');assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));{ const shot = {path:path.join(output,`${name}-${lang}-receipt.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
     await query('UPDATE email_configuration SET enabled=true');
     const waitMail=async n=>{for(let i=0;i<100;i++){if((await query("SELECT count(*) FROM email_outbox WHERE event_key LIKE $1 AND state='accepted'",[`order:${order.id}:%`])).rows[0].count===String(n))return;await new Promise(r=>setTimeout(r,100));}throw Error('Order email acceptance timeout');};await waitMail(2);
     for(const[to,locale]of[[recipient,lang],[shop,primary]]){const found=await(await fetch(api+'/api/v1/search?query='+encodeURIComponent(`to:${to}`))).json();assert.equal(found.messages.length,1);const message=await(await fetch(api+'/api/v1/message/'+found.messages[0].ID)).json();assert.ok(message.Subject.includes(reference));assert.ok(!message.Subject.includes('[TEST]')&&!message.Subject.includes('[PROVA]'));assert.ok(message.HTML.includes(`lang="${locale}"`));assert.ok(message.Text.includes(locale==='it'?'13,50 EUR':'13.50 EUR'));if(payment==='1')assert.ok(message.Text.includes('IT60X0542811101000000123456'));}
     await query('UPDATE administrators SET locale=$1',[lang]);await admin.goto(origin+'/admin/orders');
     const table=admin.locator('.order-table'),row=table.locator('tbody tr').filter({has:admin.getByRole('rowheader',{name:reference,exact:true})});
     assert.deepEqual(await table.locator('thead th .ui-table__sort-label,thead th:last-child:not([aria-sort])').allTextContents(),lang==='it'?['Numero ordine','Creato il','Cliente','Stato ordine','Pagamento','Totale','Azioni']:['Order number','Created at','Customer','Order state','Payment','Total','Actions']);
     assert.equal(await row.locator('td').count(),6); assert.ok((await row.innerText()).includes(lang==='it'?'13,50 EUR':'13.50 EUR'));
     assert.ok((await row.locator('td').nth(3).innerText()).includes(lang==='it'?'Non pagato':'Unpaid'));
     assert.match(await row.locator('td').nth(0).innerText(),/^\d{2}\/\d{2}\/\d{4} \d{2}:\d{2}:\d{2}$/);
     assert.ok((await row.locator('td').nth(1).innerText()).includes('Alex Example'));
     assert.ok((await row.locator('td').nth(1).innerText()).includes(recipient));
     const region=admin.locator('.ui-table-region'); await region.focus();
     assert.ok(await region.evaluate(el=>el===document.activeElement&&getComputedStyle(el).outlineStyle!=='none'));
     if(width<940){
      await admin.keyboard.press('ArrowRight');
      for(let i=0;i<30&&await region.evaluate(el=>el.scrollLeft===0);i++)await new Promise(r=>setTimeout(r,30));
      assert.ok(await region.evaluate(el=>el.scrollLeft>0),'Keyboard scroll works without JavaScript');
     }else assert.ok(await region.evaluate(el=>el.scrollWidth<=el.clientWidth+1),'All columns fit on desktop');
     assert.ok(await admin.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
     await admin.screenshot({path:path.join(output,`${name}-${lang}-list.png`),fullPage:true});
     const action=row.getByRole('link',{name:(lang==='it'?'Consulta ':'View ')+reference,exact:true}); await action.focus(); assert.ok(await action.evaluate(el=>el===document.activeElement)); await admin.keyboard.press('Enter'); await admin.waitForURL(origin+`/admin/orders/${order.id}`);assert.ok(await admin.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));{ const shot = {path:path.join(output,`${name}-${lang}-admin.png`),fullPage:true}; await presentation.verify(admin, path.basename(shot.path)); await presentation.screenshot(admin, shot); }
     const cancelPath=`/admin/orders/${order.id}/cancel`,cancel=admin.locator(`form[action="${cancelPath}"]`);await cancel.locator('[name=confirm]').check();const again=await cancel.evaluate(form=>Object.fromEntries(new FormData(form)));const done=admin.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname===cancelPath);await cancel.locator('button').click();assert.equal((await done).status(),303);await admin.waitForLoadState('load');await waitMail(4);
     for(const[to,locale]of[[recipient,lang],[shop,primary]]){const found=await(await fetch(api+'/api/v1/search?query='+encodeURIComponent(`to:${to}`))).json();assert.equal(found.messages.length,2);let cancelled=false;for(const captured of found.messages){const message=await(await fetch(api+'/api/v1/message/'+captured.ID)).json();assert.ok(message.Subject.includes(reference));assert.ok(message.HTML.includes(`lang="${locale}"`));if(message.Subject.includes(locale==='it'?(to===recipient?'Ordine annullato':'Annullamento ordine'):(to===recipient?'Order cancelled':'Order cancellation')))cancelled=true;}assert.ok(cancelled,'Cancellation notification captured');}
     assert.equal((await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock,10);
     assert.equal((await adminContext.request.post(origin+`/admin/catalog/products/${product}`,{form:beforeRestore,maxRedirects:0})).status(),409);assert.equal((await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock,10);
     assert.equal((await adminContext.request.post(origin+cancelPath,{form:again,maxRedirects:0})).status(),303);assert.equal((await query('SELECT count(*) FROM email_outbox WHERE event_key LIKE $1',[`order:${order.id}:%`])).rows[0].count,'4');assert.equal((await query("SELECT count(*) FROM order_stock_moves WHERE kind='credit'")).rows[0].count,lang==='it'?'1':'2');
     await page.goto(receiptUrl);const text=await page.locator('main').innerText();assert.ok(text.includes(lang==='it'?'Ordine annullato':'Order cancelled'));assert.ok(!text.includes(lang==='it'?'Usa questo riferimento per il pagamento:':'Use this payment reference:'));{ const shot = {path:path.join(output,`${name}-${lang}-cancelled.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
     await admin.goto(origin+'/admin/orders');
     const cancelledRow=admin.locator('.order-table tbody tr').filter({has:admin.getByRole('rowheader',{name:reference,exact:true})});
     assert.equal(await cancelledRow.locator('.ui-status-badge').innerText(),lang==='it'?'Annullato':'Cancelled');
     await admin.goto(origin+'/admin/settings/email');assert.ok((await admin.locator('main').innerText()).includes(lang==='it'?'Accettato':'Accepted'));
    }finally{await context.close();}
   }
   // Full manual receipts use the real forms, paused outbox and local SMTP.
   for(const lang of ['it','en']){
    const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js}),page=await context.newPage();page.on('pageerror',e=>errors.push(e.message));
    const prefix=lang===primary?'':`/${lang}`,route=prefix+'/checkout',recipient=`paid-${tag}-${lang}@example.test`,shop=`paid-shop-${tag}-${lang}@example.test`,paymentReference=`PAY-${tag}-${lang}-`.padEnd(160,'R');
    try{
     await query('UPDATE administrators SET locale=$1',[lang]);await query('UPDATE email_configuration SET enabled=false,admin_recipients=ARRAY[$1::text]',[shop]);
     const beforeStock=(await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock;
     await page.goto(origin+prefix+'/products/order-product');await page.getByRole('button',{name:lang==='it'?'Aggiungi al carrello':'Add to cart',exact:true}).click();await page.goto(origin+route);
     for(const[k,v]of Object.entries({email:recipient,first_name:'Alex',last_name:'Example',phone:'+393331234567',shipping_street:'Via Example',shipping_number:'1',shipping_postal:'00100',shipping_city:'Roma',shipping_region:'RM'}))await page.locator('#'+k).fill(v);await page.locator('#shipping_country').selectOption('IT');
     const submit=async(action,status)=>{
       if(action==='preview'&&new URL(page.url()).pathname===route){const done=page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname===route);await page.locator('button[value=continue]').click();assert.equal((await done).status(),303);await page.waitForURL(origin+route+'/methods');return;}
       if(action==='preview'&&js){await page.waitForFunction(()=>!document.querySelector('#checkout-methods-region[aria-busy=true]'));return;}
       const [response]=await Promise.all([page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname===route+'/methods'),page.waitForNavigation({waitUntil:'load'}),page.locator(`button[value=${action==='preview'?'update':action}]`).click()]);assert.equal(response.status(),action==='preview'?303:status);
     };
     await submit('preview',200);await page.locator('[name=shipping_method_id][value="1"]').check();await submit('preview',200);await page.locator(`[name=payment_method_id][value="${lang==='it'?'1':'2'}"]`).check();await submit('preview',200);await submit('confirm',303);await page.waitForURL('**/checkout/orders/ORD-*');
     const receiptUrl=page.url(),reference=new URL(receiptUrl).pathname.split('/').pop(),order=(await query('SELECT id,revision,total_cents,snapshot FROM orders WHERE reference=$1',[reference])).rows[0],paymentPath=`/admin/orders/${order.id}/payment`;
     await admin.goto(origin+`/admin/orders/${order.id}`);await admin.locator('#payment-reference').focus();await admin.keyboard.press('Tab');assert.equal(await admin.locator(`#manual-payment [name=confirm]`).evaluate(el=>el===document.activeElement),true);
     const pay=async(status)=>{const done=admin.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname===paymentPath);await admin.locator('#manual-payment button[type=submit]').click();assert.equal((await done).status(),status);await admin.waitForLoadState('load');};
     await admin.locator('#payment-reference').fill('   ');await admin.locator('#manual-payment [name=confirm]').check();await pay(422);assert.equal(await admin.locator('#payment-reference').getAttribute('aria-invalid'),'true');assert.ok(await admin.locator('#payment-reference-error').isVisible());assert.equal(await admin.locator('#payment-reference').inputValue(),'   ');
     await admin.locator('#payment-reference').fill(paymentReference);await admin.locator('#manual-payment [name=confirm]').check();const replay=await admin.locator('#manual-payment form').evaluate(form=>Object.fromEntries(new FormData(form)));{ const shot = {path:path.join(output,`${name}-${lang}-payment-form.png`),fullPage:true}; await presentation.verify(admin, path.basename(shot.path)); await presentation.screenshot(admin, shot); }await pay(303);await admin.waitForURL(origin+`/admin/orders/${order.id}?payment=1`);
     assert.equal(await admin.locator('#manual-payment-history').count(),1);assert.equal(await admin.locator('#manual-payment').count(),0);assert.equal(await admin.locator(`form[action="/admin/orders/${order.id}/cancel"]`).count(),0);assert.ok((await admin.locator('#manual-payment-history').innerText()).includes(paymentReference));
     assert.ok(await admin.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));{ const shot = {path:path.join(output,`${name}-${lang}-payment-history.png`),fullPage:true}; await presentation.verify(admin, path.basename(shot.path)); await presentation.screenshot(admin, shot); }
     assert.equal((await adminContext.request.post(origin+paymentPath,{form:replay,maxRedirects:0})).status(),303);assert.equal((await adminContext.request.post(origin+paymentPath,{form:{...replay,reference:'DIFFERENT'},maxRedirects:0})).status(),409);
     assert.equal((await adminContext.request.post(origin+`/admin/orders/${order.id}/cancel`,{form:{csrf:replay.csrf,revision:'1',confirm:'true'},maxRedirects:0})).status(),409);
     let stored=(await query('SELECT amount_cents,currency,reference,administrator_id FROM order_manual_payments WHERE order_id=$1',[order.id])).rows;assert.equal(stored.length,1);assert.equal(stored[0].amount_cents,order.total_cents);assert.equal(stored[0].currency,'EUR');assert.equal((await query('SELECT snapshot FROM orders WHERE id=$1',[order.id])).rows[0].snapshot,order.snapshot);assert.equal((await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock,beforeStock-1);
     assert.equal((await query("SELECT state FROM email_outbox WHERE order_id=$1 AND template='order_received'",[order.id])).rows[0].state,'superseded');
     await stop();cli('doctor');await start();await admin.goto(origin+`/admin/orders/${order.id}`);assert.equal(await admin.locator('#manual-payment-history').count(),1);await page.goto(receiptUrl);assert.ok((await page.locator('#order-payment-state').innerText()).includes(lang==='it'?'Pagato':'Paid'));assert.ok(!(await page.locator('main').innerText()).includes(lang==='it'?'Usa questo riferimento per il pagamento:':'Use this payment reference:'));assert.ok(!(await page.locator('main').innerText()).includes(paymentReference));assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));{ const shot = {path:path.join(output,`${name}-${lang}-paid-receipt.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
     await query('UPDATE email_configuration SET enabled=true');for(let i=0;i<100;i++){if((await query("SELECT count(*) FROM email_outbox WHERE order_id=$1 AND state='accepted'",[order.id])).rows[0].count==='2')break;await new Promise(r=>setTimeout(r,100));}
     assert.equal((await query("SELECT count(*) FROM email_outbox WHERE order_id=$1 AND state='accepted'",[order.id])).rows[0].count,'2');
     for(const[to,locale]of[[recipient,lang],[shop,primary]]){const found=await(await fetch(api+'/api/v1/search?query='+encodeURIComponent(`to:${to}`))).json();assert.equal(found.messages.length,1);const message=await(await fetch(api+'/api/v1/message/'+found.messages[0].ID)).json();assert.ok(message.Subject.includes(reference));assert.ok(message.HTML.includes(`lang="${locale}"`));if(to===recipient){assert.ok(message.Subject.includes(lang==='it'?'Pagamento registrato':'Payment recorded'));assert.ok(message.Text.includes(lang==='it'?'Pagamento registrato':'Payment recorded'));assert.ok(!message.Text.includes('IBAN:'));}}
     await admin.goto(origin+'/admin/orders');
     const paidRow=admin.locator('.order-table tbody tr').filter({has:admin.getByRole('rowheader',{name:reference,exact:true})});
     assert.equal(await paidRow.locator('td').nth(3).innerText(),lang==='it'?'Pagato':'Paid');
     await admin.screenshot({path:path.join(output,`${name}-${lang}-list-states.png`),fullPage:true});
    }finally{await context.close();}
   }
   await require('./browser-order-filters.cjs')({page:admin,query,origin,name,output,js});
   await require('./browser-current-order-status.cjs')({page:admin,query,origin,name,output});
   // The initial, empty email configuration permits checkout without notifications.
   await query("UPDATE email_configuration SET enabled=false,from_name='',from_email='',reply_to='',admin_recipients='{}'");
   const mailBefore=(await query('SELECT count(*)::integer AS total FROM email_outbox')).rows[0].total;
   for(const lang of ['it','en']){
    const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js}),page=await context.newPage();page.on('pageerror',e=>errors.push(e.message));
    const prefix=lang===primary?'':`/${lang}`,route=prefix+'/checkout';
    try{
     const stockBefore=(await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock;
     await page.goto(origin+prefix+'/products/order-product');await page.getByRole('button',{name:lang==='it'?'Aggiungi al carrello':'Add to cart',exact:true}).click();await page.goto(origin+route);
     for(const[k,v]of Object.entries({email:`no-mail-${tag}-${lang}@example.test`,first_name:'Alex',last_name:'Example',phone:'+393331234567',shipping_street:'Via Example',shipping_number:'1',shipping_postal:'00100',shipping_city:'Roma',shipping_region:'RM'}))await page.locator('#'+k).fill(v);
     await page.locator('#shipping_country').selectOption('IT');
     await Promise.all([page.waitForURL(origin+route+'/methods'),page.locator('button[value=continue]').click()]);
     const refresh=async()=>{
      if(js){await page.waitForFunction(()=>!document.querySelector('#checkout-methods-region[aria-busy=true]'));return;}
      await Promise.all([page.waitForNavigation({waitUntil:'load'}),page.locator('button[value=update]').click()]);
     };
     await page.locator('[name=shipping_method_id][value="1"]').check();await refresh();
     await page.locator('[name=payment_method_id][value="2"]').check();await refresh();
     const replay=await page.locator('#checkout-methods').evaluate(form=>Object.fromEntries(new FormData(form)));replay.action='confirm';
     const [response]=await Promise.all([page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname===route+'/methods'),page.waitForNavigation({waitUntil:'load'}),page.locator('button[value=confirm]').click()]);
     assert.equal(response.status(),303);await page.waitForURL('**/checkout/orders/ORD-*');
     const reference=new URL(page.url()).pathname.split('/').pop(),order=(await query('SELECT id FROM orders WHERE reference=$1',[reference])).rows[0];
     assert.ok(order);assert.equal(await page.locator('html').getAttribute('lang'),lang);
     assert.ok(!(await page.locator('main').innerText()).includes(lang==='it'?'Ti invieremo il riepilogo anche via email.':'We will also email your order summary.'),'No promise of an email when notifications were not created');
     assert.equal((await query('SELECT stock FROM catalog_products WHERE id=$1',[product])).rows[0].stock,stockBefore-1);
     assert.equal((await query('SELECT count(*) FROM order_stock_moves m JOIN order_lines l ON l.id=m.line_id WHERE l.order_id=$1',[order.id])).rows[0].count,'1');
     assert.equal((await query('SELECT count(*)::integer AS total FROM email_outbox')).rows[0].total,mailBefore);
     assert.equal((await context.request.post(origin+route+'/methods',{form:replay,maxRedirects:0})).status(),303);
     assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
     await page.screenshot({path:path.join(output,`${name}-${lang}-unconfigured-email-receipt.png`),fullPage:true});
    }finally{await context.close();}
   }
   assert.deepEqual(errors,[]);results.push({name,languages:['it','en'],javascript:js,captured:12,passed:true});console.log(name,'passed');
  }finally{await stop();await adminContext.close();await query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);fs.rmSync(folder,{recursive:true,force:true});}
 }}finally{await browser.close();await db.end();}
 fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
})().catch(e=>{console.error(e);process.exitCode=1;});
