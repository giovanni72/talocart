const presentation = require('./theme-presentation-helper.cjs');
// Invoked by the Rust fixture: synthetic accounts, isolated PostgreSQL and media, no external email.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {chromium}=require('playwright');
(async()=>{
 const {origin,pendingProfiles}=JSON.parse(fs.readFileSync(0,'utf8'));
 const output=path.resolve('.local/verification/customers/browser');fs.mkdirSync(output,{recursive:true});
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{}),results=[];
 const secret='synthetic customer password';
 try{for(const[profile,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]])for(const locale of ['en','it']){
  const prefix=locale==='it'?'/it':'',url=p=>origin+prefix+p,context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
  const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
  const email=`${profile}-${locale}@example.test`;
  const fill=async values=>{for(const[name,value]of Object.entries(values))await page.locator(`#customer-${name}`).fill(value);};
  const submit=async(expected=303)=>{const pending=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().startsWith(origin));if(!js&&new URL(page.url()).pathname.endsWith('/account/register'))await page.locator('#customer-email').press('Enter');else await page.locator('#customer-form button[type=submit]:not([value=refresh])').click();assert.equal((await pending).status(),expected);await page.waitForLoadState('load');};
  const checkLayout=async()=>assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),`${profile}-${locale}: no overflow`);
  const snapshot=async name=>{await checkLayout();{ const shot = {path:path.join(output,`${profile}-${locale}-${name}.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }};
  try{
   await page.goto(url('/products/method-item'));await page.locator('.store-cart-add button').click();await page.waitForURL(u=>u.pathname.endsWith('/cart'));
   const guestBefore=(await context.cookies()).find(c=>c.name==='talocart_cart').value;
   await page.goto(url('/checkout'));await page.waitForURL(url('/account/login?next=%2Fcheckout'));assert.equal(await page.locator('#customer-checkout-login').count(),1);await page.locator('.customer-navigation a[href*="/account/register"]').click();const anonymous=(await context.cookies()).find(c=>c.name==='talocart_customer').value;
   await fill({first_name:'Alex',last_name:'Example',email:'not-an-email',password:secret,password_confirmation:secret});await submit(422);
   assert.equal(await page.locator('#customer-email').getAttribute('aria-invalid'),'true');assert.equal(await page.locator('#customer-first_name').inputValue(),'Alex');assert.equal(await page.locator('#customer-password').inputValue(),'');
   await fill({email,password:secret,password_confirmation:secret});await submit();await page.waitForURL(url('/checkout'));
   const signed=(await context.cookies()).find(c=>c.name==='talocart_customer');assert.notEqual(signed.value,anonymous);assert.equal(signed.httpOnly,true);assert.equal(signed.sameSite,'Lax');assert.equal((await context.cookies()).find(c=>c.name==='talocart_cart').value,guestBefore);
   await page.goto(url('/account'));
   assert.equal(await page.locator('#customer-email').count(),0);assert.equal(await page.locator('#customer-current_password').count(),0);
   await fill({first_name:'Alex',last_name:'Example'});await submit();await page.waitForURL(url('/account?saved=1'));await page.reload();assert.equal((await page.locator('#customer-notice').count()),1);
   await snapshot('profile');
   await page.goto(url('/account/addresses/new'));
   await fill({label:'Home <safe>',first_name:'Alex',last_name:'Example',phone:'+39 3331234567',street:'Via Example',number:'1',postal:'bad',city:'Roma',region:'RM'});await page.locator('#customer-country').selectOption('IT');await page.locator('#customer-is_default').check();await submit(422);assert.equal(await page.locator('#customer-is_default').isChecked(),true);
   assert.equal(await page.locator('#customer-postal').getAttribute('aria-invalid'),'true');assert.equal(await page.locator('#customer-street').inputValue(),'Via Example');await fill({postal:'00100'});await submit();await page.waitForURL(url('/account/addresses'));
   assert.equal(await page.locator('[data-customer-address]').count(),1);assert.equal(await page.locator('main safe').count(),0);
   const addressId=await page.locator('[data-customer-address]').getAttribute('data-customer-address');
   assert.equal(await page.locator('[data-address-default]').count(),1);
   await page.goto(url('/checkout'));assert.equal(await page.locator('#shipping_street').inputValue(),'Via Example');assert.equal(await page.locator('#phone').inputValue(),'+39 3331234567');assert.equal(await page.locator('#checkout-address').inputValue(),addressId);
   await snapshot('checkout-default-address');await page.goto(url('/account/addresses'));
   const stale=await context.newPage();await stale.goto(url('/account/addresses/'+addressId));await page.goto(url('/account/addresses/'+addressId));await fill({label:'Updated home'});await submit();
   const old=stale.waitForResponse(r=>r.request().method()==='POST');await stale.locator('#customer-form button[type=submit]:not([value=refresh])').click();assert.equal((await old).status(),409);
   // Dispose this tab with its context: closing the rejected navigation's target
   // individually can strand Chromium's Page.close acknowledgement.
   await snapshot('addresses');
   await page.locator('form[action$="/use"] button').focus();await page.keyboard.press('Enter');await page.waitForURL(url('/checkout'));
   assert.equal(await page.locator('#email').inputValue(),email);assert.equal(await page.locator('#shipping_street').inputValue(),'Via Example');assert.equal(await page.locator('#first_name').inputValue(),'Alex');
   // Choose a saved address directly in checkout, retaining unsaved notes/fiscal fields.
   assert.equal(await page.locator('#checkout-address option').count(),2);
   const addressSelectBox=await page.locator('#checkout-address').boundingBox(),addressButtonBox=await page.locator('button[value=address]').boundingBox();assert.ok(addressButtonBox.y-addressSelectBox.y-addressSelectBox.height>=16,'Saved address action is spaced from the select');assert.ok(addressButtonBox.height<=40,'Saved address action uses the compact button size');
   assert.equal(await page.locator('#phone').inputValue(),'+39 3331234567');
   await page.locator('#delivery_notes').fill('Keep these delivery notes');
   await page.locator('#shipping_street').fill('Unsaved street');
   await page.locator('#checkout-address').selectOption(addressId);
   await page.locator('button[value=address]').focus();
   const [addressResponse]=await Promise.all([page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname.endsWith('/checkout')),page.waitForNavigation({waitUntil:'load'}),page.keyboard.press('Enter')]);
   assert.equal(addressResponse.status(),200);
   assert.equal(await page.locator('#shipping_street').inputValue(),'Via Example');
   assert.equal(await page.locator('#phone').inputValue(),'+39 3331234567');
   assert.equal(await page.locator('#delivery_notes').inputValue(),'Keep these delivery notes');
   assert.equal(await page.locator('#first_name').inputValue(),'Alex');
   await snapshot('checkout-address');
   {const clip=await page.locator('section[aria-labelledby=checkout-address-title]').boundingBox();await presentation.screenshot(page,{path:path.join(output,`${profile}-${locale}-address-controls.png`),clip});}
   // Enter in a text field must continue, never implicitly reapply the saved address.
   await page.locator('#shipping_street').fill('Edited checkout street');await page.locator('#phone').fill('+39 3337654321');await page.locator('#email').fill('invalid');
   const [invalidContinue]=await Promise.all([page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname.endsWith('/checkout')),page.waitForNavigation({waitUntil:'load'}),page.locator('#shipping_street').press('Enter')]);
   assert.equal(new URLSearchParams(invalidContinue.request().postData()).get('action'),'continue','Enter must submit Continue even with a saved address selected');assert.equal(invalidContinue.status(),422);
   assert.equal(await page.locator('#shipping_street').inputValue(),'Edited checkout street');assert.equal(await page.locator('#phone').inputValue(),'+39 3337654321');
   await page.locator('#email').fill(email);await page.locator('#checkout-address').selectOption('');
   const [continued]=await Promise.all([page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname.endsWith('/checkout')),page.waitForURL(url('/checkout/methods')),page.locator('#phone').press('Enter')]);
   assert.equal(new URLSearchParams(continued.request().postData()).get('action'),'continue');assert.equal(continued.status(),303);
   await page.goto(url('/checkout'));assert.equal(await page.locator('#shipping_street').inputValue(),'Edited checkout street');assert.equal(await page.locator('#phone').inputValue(),'+39 3337654321');
   await page.locator('button[value=continue]').click();await page.waitForURL(url('/checkout/methods'));
   await page.locator('input[name=shipping_method_id][value="1"]').check();await page.locator('input[name=payment_method_id][value="1"]').check();
   if(js)await page.waitForFunction(()=>!document.querySelector('#checkout-confirm').disabled);else{await page.locator('button[value=update]').click();await page.waitForLoadState('load');}
   const expected=await page.locator('#checkout-methods input[name=customer_id]').inputValue();assert.ok(Number(expected)>0);
   await page.locator('#checkout-confirm').click();await page.waitForURL(/\/checkout\/orders\/ORD-/);const reference=new URL(page.url()).pathname.split('/').pop();
   await page.goto(url('/account/orders'));assert.equal(await page.locator('a[href$="/'+reference+'"]').count(),1);await snapshot('history');await page.locator('a[href$="/'+reference+'"]').click();assert.ok((await page.locator('main').innerText()).includes('Edited checkout street'));await snapshot('order');
   await page.reload();assert.ok((await page.locator('main').innerText()).includes(reference));
   const stranger=await browser.newContext({javaScriptEnabled:js}),other=await stranger.newPage();await other.goto(url('/account/register'));
   for(const[name,value]of Object.entries({first_name:'Other',last_name:'Buyer',email:`other-${email}`,password:secret,password_confirmation:secret}))await other.locator('#customer-'+name).fill(value);
   await other.locator('#customer-form button:not([value=refresh])').click();await other.waitForURL(url('/account'));
   assert.equal((await other.goto(url('/account/orders/'+reference))).status(),404);assert.equal((await other.goto(url('/account/addresses/'+addressId))).status(),404);await stranger.close();
   await page.goto(url('/account/addresses'));await page.locator('.customer-delete > summary').click();await page.locator('input[name=confirm]').check();await page.locator('form[action$="/delete"] button').click();await page.waitForLoadState('load');assert.equal(await page.locator('[data-customer-address]').count(),0);
   await page.goto(url('/account/orders/'+reference));assert.ok((await page.locator('main').innerText()).includes('Edited checkout street'));
   await page.goto(url('/products/method-item'));await page.locator('.store-cart-add input[name=quantity]').fill('2');await page.locator('.store-cart-add button').click();await page.waitForURL(u=>u.pathname.endsWith('/cart'));
   await page.goto(url('/account'));
   await page.locator('form[action$="/logout"] button').click();await page.waitForURL(url('/account/login'));
   assert.equal((await context.cookies()).find(c=>c.name==='talocart_cart'),undefined);
   await page.goto(url('/products/method-item'));await page.locator('.store-cart-add input[name=quantity]').fill('3');await page.locator('.store-cart-add button').click();await page.waitForURL(u=>u.pathname.endsWith('/cart'));
   await page.goto(url('/account/orders'));await page.waitForURL(u=>u.pathname.endsWith('/account/login')&&u.searchParams.get('next')==='/account/orders');
   await fill({email,password:'incorrect'});await submit(401);assert.equal(await page.locator('#customer-password').inputValue(),'');await fill({email,password:secret});await submit();await page.waitForURL(url('/account/orders'));
   await page.goto(url('/cart'));assert.equal(await page.locator('.cart-item input[name=quantity]').inputValue(),'5');await page.reload();assert.equal(await page.locator('.cart-item input[name=quantity]').inputValue(),'5');await snapshot('persistent-cart');
   const device=await browser.newContext({javaScriptEnabled:js}),devicePage=await device.newPage();await devicePage.goto(url('/account/login'));
   await devicePage.locator('#customer-email').fill(email);await devicePage.locator('#customer-password').fill(secret);await devicePage.locator('#customer-form button:not([value=refresh])').click();await devicePage.waitForURL(url('/account'));
   await devicePage.goto(url('/cart'));assert.equal(await devicePage.locator('.cart-item input[name=quantity]').inputValue(),'5');await devicePage.goto(url('/account/orders/'+reference));await devicePage.locator('#customer-order-receipt').click();await devicePage.waitForURL(url('/checkout/orders/'+reference));assert.ok((await devicePage.locator('main').innerText()).includes(reference));await device.close();
   await page.goto(url('/account/password'));await fill({current_password:secret,password:'updated customer password',password_confirmation:'different password'});await submit(422);assert.equal(await page.locator('#customer-password_confirmation').getAttribute('aria-invalid'),'true');
   await fill({current_password:secret,password:'updated customer password',password_confirmation:'updated customer password'});await submit();await page.waitForURL(url('/account/login?changed=password'));assert.equal(await page.locator('#customer-notice').count(),1);
   await fill({email,password:'updated customer password'});await submit();await page.goto(url('/account/orders'));assert.ok((await page.locator('main').innerText()).includes(reference));
   await page.goto(url('/account/email'));await fill({email:`changed-${email}`,current_password:'wrong'});await submit(422);
   assert.equal(await page.locator('#customer-current_password').getAttribute('aria-invalid'),'true');await snapshot('email-error');
   await fill({current_password:'updated customer password'});await submit();await page.waitForURL(url('/account/login?changed=email'));assert.equal(await page.locator('#customer-notice').count(),1);
   await fill({email:`changed-${email}`,password:'updated customer password'});await submit();await page.waitForURL(url('/account'));await page.goto(url('/account/orders'));assert.ok((await page.locator('main').innerText()).includes(reference));
   const pendingFixture=pendingProfiles[results.length],pendingContext=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
   await pendingContext.addCookies([{name:'talocart_cart',value:pendingFixture.guest,url:origin,httpOnly:true,sameSite:'Lax'}]);
   const pendingPage=await pendingContext.newPage();await pendingPage.goto(url('/account/login'));
   await pendingPage.locator('#customer-email').fill(pendingFixture.email);await pendingPage.locator('#customer-password').fill(secret);await pendingPage.locator('#customer-form button:not([value=refresh])').click();await pendingPage.waitForURL(url('/cart'));
   assert.equal(await pendingPage.locator('#cart-merge-title').count(),1);await pendingPage.locator('section[aria-labelledby=cart-merge-title] summary').click();
   assert.ok(await pendingPage.locator('section[aria-labelledby=cart-merge-title] li').innerText());
   assert.ok(await pendingPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));{ const shot = {path:path.join(output,`${profile}-${locale}-merge-pending.png`),fullPage:true}; await presentation.verify(pendingPage, path.basename(shot.path)); await presentation.screenshot(pendingPage, shot); }
   await pendingPage.locator('.cart-item input[name=quantity]').fill('2');await pendingPage.locator('.cart-item button[value=update]').click();await pendingPage.waitForLoadState('load');
   await pendingPage.locator('form[action$="/cart/merge"] button').focus();const mergeResponse=pendingPage.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname.endsWith('/cart/merge'));await pendingPage.keyboard.press('Enter');assert.equal((await mergeResponse).status(),303);await pendingPage.locator('#cart-merge-title').waitFor({state:'detached'});await pendingPage.reload();
   assert.equal(await pendingPage.locator('#cart-merge-title').count(),0);assert.equal(await pendingPage.locator('.cart-item input[name=quantity]').inputValue(),'3');await pendingContext.close();
   await checkLayout();assert.deepEqual(errors,[]);results.push({profile,locale,javascript:js,passed:true});
  }finally{await context.close();}
 }}finally{await browser.close();}
 fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));console.log(JSON.stringify(results));
})().catch(e=>{console.error(e.stack);process.exit(1);});
