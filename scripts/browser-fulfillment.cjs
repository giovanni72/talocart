const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),{chromium}=require('playwright');
(async()=>{
 const {origin,admin,customer,profiles}=JSON.parse(fs.readFileSync(0,'utf8'));
 const output=path.resolve('.local/verification/fulfillment-consolidation/browser');fs.mkdirSync(output,{recursive:true});
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{}),results=[];let index=0;
 try{for(const [profile,width,js] of [['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]])for(const locale of ['en','it']){
  const {id,reference}=profiles[index++],prefix=locale==='it'?'/it':'',context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
  await context.addCookies([{name:'talocart_session',value:admin,url:origin},{name:'talocart_customer',value:customer,url:origin}]);const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
  const openEditor=async(kind,target=page)=>{const d=target.locator('#fulfillment-'+kind+'-editor');if(await d.count() && !(await d.evaluate(e=>e.open)))await d.locator('summary').click();};
  const url=origin+'/admin/orders/'+id,submit=async(selector,status=303)=>{const wait=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().endsWith('/fulfillment'));await page.locator(selector).focus();await page.keyboard.press('Enter');assert.equal((await wait).status(),status);await page.waitForLoadState('load');};
  const shot=async(name)=>{assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),`${profile}-${locale}-${name} overflow`);await page.screenshot({path:path.join(output,`${profile}-${locale}-${name}.png`),fullPage:true});};
  try{
   await page.goto(origin+'/admin/account');await page.locator('#account-language').selectOption(locale);await page.locator('form[action="/admin/account/language"] button').click();await page.waitForLoadState('load');await page.goto(url);assert.equal(await page.locator('html').getAttribute('lang'),locale);
   await openEditor('note');await page.locator('#fulfillment-body').fill('PRIVATE-NOTE <script>window.injected=true</script>\nInternal only');await submit('#fulfillment-note button');await page.waitForURL(u=>u.searchParams.get('fulfillment')==='1');assert.equal(await page.locator('[data-private-note]').count(),1);assert.equal(await page.locator('main script').count(),0);
   const stale=await context.newPage();await stale.goto(url);await openEditor('note',stale);await stale.locator('#fulfillment-body').fill('PRIVATE-NOTE stale draft');
   await submit('#fulfillment-transition button[value=preparing]');await page.waitForLoadState('load');const response=stale.waitForResponse(r=>r.request().method()==='POST');await stale.locator('#fulfillment-note button').click();assert.equal((await response).status(),409);await stale.waitForLoadState('load');assert.equal(await stale.locator('#fulfillment-note').count(),0);await stale.locator('#fulfillment-submitted > summary').click();assert.ok((await stale.locator('main').innerText()).includes('PRIVATE-NOTE stale draft'));await stale.close();
   await page.locator('#fulfillment-carrier').fill('C'.repeat(80));await openEditor('tracking');await page.locator('#fulfillment-tracking_number').fill('TRACK-1');await page.locator('#fulfillment-tracking_url').fill('javascript:alert(1)');await submit('#fulfillment-transition button[value=shipped]',422);assert.equal(await page.locator('#fulfillment-tracking_url').getAttribute('aria-invalid'),'true');assert.equal(await page.locator('#fulfillment-carrier').inputValue(),'C'.repeat(80));
   await page.locator('#fulfillment-tracking_url').fill('https://example.test/track/1');await submit('#fulfillment-transition button[value=shipped]');await page.waitForLoadState('load');assert.equal(await page.locator('[data-order-status-role]').getAttribute('data-order-status-role'),'shipped');assert.equal(await page.locator('form[action$="/cancel"]').count(),0);await shot('shipped');
   await openEditor('tracking');await page.locator('#fulfillment-tracking_number').fill('UNSAVED-TRACKING');
   await openEditor('note');await page.locator('#fulfillment-body').fill('UNSAVED-NOTE');
   await submit('#fulfillment-delivery button',409);await page.waitForLoadState('load');
   assert.equal(await page.locator('#fulfillment-tracking_number').inputValue(),'UNSAVED-TRACKING');
   assert.equal(await page.locator('#fulfillment-body').inputValue(),'UNSAVED-NOTE');
   assert.equal(await page.locator('[data-order-status-role]').first().getAttribute('data-order-status-role'),'shipped');
   await submit('#fulfillment-note button',200);await page.waitForLoadState('load');
   assert.equal(await page.locator('#fulfillment-body').inputValue(),'');
   assert.equal(await page.locator('#fulfillment-tracking_number').inputValue(),'UNSAVED-TRACKING');
   await openEditor('note');await page.locator('#fulfillment-body').fill('SECOND-DRAFT');
   await submit('#fulfillment-tracking button',200);await page.waitForLoadState('load');
   assert.equal(await page.locator('#fulfillment-body').inputValue(),'SECOND-DRAFT');
   await openEditor('tracking');await page.locator('#fulfillment-tracking_number').fill('TRACK-1');
   await submit('#fulfillment-tracking button',200);await page.waitForLoadState('load');
   await openEditor('note');await page.locator('#fulfillment-body').fill('');

   await openEditor('note');await page.locator('#fulfillment-body').fill('PRIVATE-NOTE after shipment');await submit('#fulfillment-note button');await page.waitForLoadState('load');assert.equal(await page.locator('#fulfillment-tracking_number').inputValue(),'TRACK-1');
   await openEditor('tracking');await page.locator('#fulfillment-tracking_number').fill('CORRECTED');await page.locator('#fulfillment-tracking_url').fill('https://example.test/track/corrected');await submit('#fulfillment-tracking button[value=tracking]');await page.waitForLoadState('load');await submit('#fulfillment-delivery button');await page.waitForLoadState('load');assert.equal(await page.locator('[data-order-status-role]').getAttribute('data-order-status-role'),'delivered');await shot('admin');
   await page.goto(origin+prefix+'/checkout/orders/'+reference);assert.equal(await page.locator('[data-order-status-role]').getAttribute('data-order-status-role'),'delivered');assert.equal(await page.locator('[data-order-tracking]').getAttribute('href'),'https://example.test/track/corrected');assert.ok(!(await page.locator('main').innerText()).includes('PRIVATE-NOTE'));await shot('receipt');
   await page.goto(origin+prefix+'/account/orders/'+reference);assert.equal(await page.locator('[data-order-status-role]').getAttribute('data-order-status-role'),'delivered');assert.ok(!(await page.locator('main').innerText()).includes('PRIVATE-NOTE'));await shot('account');await page.goto(origin+prefix+'/account/orders');assert.ok((await page.locator('main').innerText()).includes(locale==='it'?'Consegnato':'Delivered'));assert.deepEqual(errors,[]);results.push({profile,locale,passed:true});
  }finally{await context.close();}
 }}finally{await browser.close();}fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));console.log(JSON.stringify(results));
})().catch(e=>{console.error(e);process.exitCode=1});
