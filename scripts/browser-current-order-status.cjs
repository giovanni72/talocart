// Runs inside the isolated orders suite; exercises actual native forms in both languages.
const assert=require('node:assert/strict'),path=require('node:path');
module.exports=async({page,query,origin,name,output})=>{
 const orders=(await query("SELECT id,reference,snapshot FROM orders WHERE payment_state='paid' ORDER BY id")).rows;
 assert.equal(orders.length,2);
 const roles=Object.fromEntries((await query('SELECT role,status_id FROM order_status_roles')).rows.map(r=>[r.role,r.status_id]));
 for(const [index,locale] of ['it','en'].entries()){
  await query('UPDATE administrators SET locale=$1',[locale]);
  await page.goto(origin+'/admin/settings/order-statuses/new');
  const it=`Ricevuto ${locale}`,en=`Received ${locale}`,color=locale==='it'?'#e11d48':'#0891b2';
  for(const [field,value] of Object.entries({name_it:it,name_en:en,color,sort_order:'7'}))await page.locator('#status-'+field).fill(value);
  await Promise.all([page.waitForNavigation({waitUntil:'load'}),page.locator('form[action$="/new"] button[type=submit]').click()]);
  const custom=(await query('SELECT id FROM order_statuses WHERE name_en=$1',[en])).rows[0].id;
  const order=orders[index],url=origin+'/admin/orders/'+order.id;
  await page.goto(url);
  const setStatus=async(id,expected=303)=>{
   await page.locator('#current-order-status').selectOption(id);
   const navigation=page.waitForNavigation({waitUntil:'load'});
   const response=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().endsWith('/status'));
   await page.locator('form[action$="/status"] button').focus();await page.keyboard.press('Enter');
   assert.equal((await response).status(),expected);await navigation;
  };
  const stale=await page.locator('form[action$="/status"]').evaluate(f=>Object.fromEntries(new FormData(f)));
  await setStatus(custom);
  const denied=await page.context().request.post(url+'/status',{form:{...stale,status_id:roles.shipped},maxRedirects:0});assert.equal(denied.status(),409);
  await page.goto(origin+'/admin/orders?state='+custom);
  assert.equal(await page.locator('.order-table tbody th[scope=row]').count(),1);
  assert.equal(await page.locator('.order-table tbody .ui-status-badge rect').getAttribute('fill'),color);
  assert.equal(await page.locator('.order-table tbody .ui-status-badge').innerText(),locale==='it'?it:en);
  for (const selector of ['.order-table thead th:nth-child(4)', '.order-table tbody td:nth-child(4)'])
   assert.equal(await page.locator(selector).first().evaluate(e=>getComputedStyle(e).textAlign),'left');
  const badge=page.locator('.order-table tbody .ui-status-badge');
  assert.equal(await badge.locator('circle').count(),0);
  assert.equal(await badge.locator('svg').evaluate(e=>e.getBoundingClientRect().width),4);
  assert.equal(await badge.evaluate(e=>getComputedStyle(e).borderTopWidth),'0px');
  assert.equal(await page.locator('#order-fulfillment').count(),0);
  assert.equal(await page.locator('.order-table thead th[scope=col]').count(),7);
  await page.screenshot({path:path.join(output,`${name}-${locale}-configured-status.png`),fullPage:true});
  await page.goto(url);await setStatus(roles.preparing);
  const operation=async(action)=>{const navigation=page.waitForNavigation({waitUntil:'load'});const response=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().endsWith('/fulfillment'));await page.locator(`#fulfillment-form button[value="${action}"]`).click();assert.equal((await response).status(),303);await navigation;};
  await page.locator('#fulfillment-carrier').fill('Status courier');
  await page.locator('#fulfillment-tracking_number').fill('TRACK-STATUS');
  await page.locator('#fulfillment-tracking_url').fill('https://example.test/status');
  await operation('shipped');
  assert.equal(await page.locator('[data-order-status-role]').getAttribute('data-order-status-role'),'shipped');
  assert.equal(await page.locator('#current-order-status').count(),0);
  await operation('delivered');
  assert.equal(await page.locator('[data-order-status-role]').getAttribute('data-order-status-role'),'delivered');
  const now=(await query('SELECT o.snapshot,o.payment_state,c.status_id,f.tracking_number FROM orders o JOIN order_current_status c ON c.order_id=o.id JOIN order_fulfillment f ON f.order_id=o.id WHERE o.id=$1',[order.id])).rows[0];
  assert.equal(now.snapshot,order.snapshot);assert.equal(now.payment_state,'paid');assert.equal(now.status_id,roles.delivered);assert.equal(now.tracking_number,'TRACK-STATUS');
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
  await page.screenshot({path:path.join(output,`${name}-${locale}-status-delivered.png`),fullPage:true});
 }
};
