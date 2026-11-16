// The real order suite supplies isolated records, including paid/cancelled guest orders.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page,query,origin,name,output,js}) => {
  const autofocusDiagnostics=[];
  page.on('console',message=>{if(/autofocus/i.test(message.text()))autofocusDiagnostics.push(message.text());});
  const records=(await query('SELECT id,reference,total_cents,payment_state,state FROM orders ORDER BY id')).rows;
  assert.equal(records.length,4);
  const list=()=>page.locator('.order-table tbody tr').filter({has:page.locator('th[scope=row]')});
  const activate=async target=>{
    await target.focus();
    await Promise.all([page.waitForNavigation({waitUntil:'load'}),page.keyboard.press('Enter')]);
  };
  const submit=()=>activate(page.locator('.order-table-search button[type=submit]'));
  const setSize=async value=>{
    if(js) await Promise.all([page.waitForNavigation({waitUntil:'load'}),page.locator('#order-page-size').selectOption(value)]);
    else {await page.locator('#order-page-size').selectOption(value);await activate(page.locator('.product-page-size button[type=submit]'));}
  };

  for(const lang of ['it','en']){
    await query('UPDATE administrators SET locale=$1',[lang]);
    await page.goto(origin+'/admin/catalog/products');
    const priceHeader=page.locator('th[aria-sort]').filter({has:page.locator('a.ui-table__sort[href*="sort=price"]')});
    await activate(priceHeader.getByRole('link'));assert.equal(await priceHeader.getAttribute('aria-sort'),'ascending');
    await activate(priceHeader.getByRole('link'));assert.equal(await priceHeader.getAttribute('aria-sort'),'descending');
    assert.deepEqual(await page.locator('#product-page-size option').allTextContents(),['10','20','50','100','500']);
    if(js) await Promise.all([page.waitForNavigation({waitUntil:'load'}),page.locator('#product-page-size').selectOption('10')]);
    else {await page.locator('#product-page-size').selectOption('10');await activate(page.locator('.product-page-size button[type=submit]'));}
    assert.equal(await page.locator('#product-page-size').inputValue(),'10');
    assert.equal(await priceHeader.getAttribute('aria-sort'),'descending');
    await page.goto(origin+'/admin/orders');
    await page.locator('#order-reference').fill(records[0].reference);
    await page.locator('#order-total').fill('abc');
    await submit();
    assert.equal(await page.locator('#order-total').inputValue(),'abc');
    assert.equal(await page.locator('#order-reference').inputValue(),records[0].reference);
    assert.equal(await page.locator('#order-total').getAttribute('aria-invalid'),'true');
    assert.equal(await page.locator('#order-total').getAttribute('aria-describedby'),'order-total-error');
    try {
      // Native autofocus also works with JavaScript disabled. Poll from Node:
      // an in-page requestAnimationFrame can stop polling in that profile.
      const deadline=Date.now()+5000;
      while(Date.now()<deadline && !(await page.locator('#order-total').evaluate(el=>document.activeElement===el))) {
        await new Promise(resolve=>setTimeout(resolve,50));
      }
      assert.equal(await page.evaluate(()=>document.activeElement.id),'order-total');
    } catch (error) {
      console.error('Order filter focus diagnostic', await page.evaluate(()=>({
        active:document.activeElement.id,documentFocused:document.hasFocus(),
        visibility:document.visibilityState,autofocus:[...document.querySelectorAll('[autofocus]')].map(el=>el.id),
      })));
      console.error('Native autofocus messages',autofocusDiagnostics);
      await page.screenshot({path:path.join(output,`${name}-${lang}-filter-focus-failure.png`),fullPage:true});
      throw error;
    }
    assert.equal(await page.evaluate(()=>document.activeElement.id),'order-total');
    assert.equal(await page.locator('.order-table').count(),1);
    assert.equal(await page.locator('#order-filter-error').count(),1);
    assert.equal(await page.locator('.shared-pagination').count(),0);
    await page.screenshot({path:path.join(output,`${name}-${lang}-filter-error.png`),fullPage:true});
    await page.locator('#order-total').fill('');await submit();assert.equal(await list().count(),1);
    await activate(page.getByRole('link',{name:lang==='it'?'Azzera filtri':'Reset filters',exact:true}));
    assert.equal(await page.locator('.order-table thead th[aria-sort]').count(),6);
    assert.equal(await page.locator('.ui-table__filters td').count(),7);
    const date=await list().first().locator('td').nth(0).innerText();assert.match(date,/^\d{2}\/\d{2}\/\d{4} \d{2}:\d{2}:\d{2}$/);
    assert.ok((await list().first().locator('td').nth(1).innerText()).includes('Alex Example'));
    await page.locator('#order-reference').fill(records[0].reference);await submit();assert.equal(await list().count(),1);
    const header=page.locator('th[aria-sort]').filter({has:page.getByText(lang==='it'?'Totale':'Total',{exact:true})});
    await activate(header.getByRole('link'));
    assert.equal(await page.locator('#order-reference').inputValue(),records[0].reference);
    assert.equal(await header.getAttribute('aria-sort'),'ascending');
    await activate(header.getByRole('link'));assert.equal(await header.getAttribute('aria-sort'),'descending');
    await activate(page.getByRole('link',{name:lang==='it'?'Azzera filtri':'Reset filters',exact:true}));
    assert.equal(await list().count(),4);assert.equal(await page.locator('#order-reference').inputValue(),'');
    await page.locator('#order-customer').fill('paid-');await page.locator('#order-payment').selectOption('paid');await page.locator('#order-state').selectOption('1');
    await page.locator('#order-total-op').selectOption('eq');await page.locator('#order-total').fill(lang==='it'?'13,50':'13.50');await submit();assert.equal(await list().count(),2);
    await page.locator('#order-total-op').selectOption('gt');await submit();assert.equal(await list().count(),0);assert.equal(await page.locator('.ui-table__filters td').count(),7);
    await page.locator('#order-total-op').selectOption('lt');await submit();assert.equal(await list().count(),0);
    await page.locator('#order-customer').fill('NO-SUCH-CUSTOMER');await submit();assert.equal(await list().count(),0);
    const empty=await page.locator('.ui-empty').innerText();assert.ok(empty.includes(lang==='it'?'Nessun risultato':'No results'));
    await page.goto(origin+'/admin/orders?reference='+encodeURIComponent(records[0].reference)+'&date=2026-01-01');assert.equal(await list().count(),0);assert.equal(await page.locator('#order-date').inputValue(),'2026-01-01');
    await page.goto(origin+'/admin/orders?sort=reference&dir=asc');
    assert.deepEqual(await list().locator('th[scope=row]').allTextContents(),records.map(r=>r.reference));
    for(const sort of ['customer','state','payment','created']){
      const response=await page.goto(origin+`/admin/orders?sort=${sort}&dir=desc`);assert.equal(response.status(),200,sort);assert.equal(await page.locator('th[aria-sort=descending]').count(),1);
    }
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    await page.screenshot({path:path.join(output,`${name}-${lang}-column-filters.png`),fullPage:true});
  }
  // Summary-only fixture rows exercise native page navigation without changing stock.
  for(let n=1;n<=22;n++){
    const id=(await query(`INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents)
      SELECT $1,$2,0,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents FROM orders WHERE id=$3 RETURNING id`,['ORD-2099-'+n,1000000000000+n,records[0].id])).rows[0].id;
    await query('INSERT INTO order_customer_summaries(order_id,first_name,last_name,email) SELECT $1,first_name,last_name,email FROM order_customer_summaries WHERE order_id=$2',[id,records[0].id]);
  }
  for(const lang of ['it','en']){
    await query('UPDATE administrators SET locale=$1',[lang]);
    await page.goto(origin+'/admin/orders?reference=ORD-2099-&state=open&payment=unpaid&sort=reference&dir=asc&per_page=20');assert.equal(await list().count(),20);
    await activate(page.locator('.shared-pagination a[rel=next]'));
    assert.equal(await list().count(),2);assert.equal(await page.locator('#order-reference').inputValue(),'ORD-2099-');assert.equal(await page.locator('#order-payment').inputValue(),'unpaid');
    assert.equal(new URL(page.url()).searchParams.get('sort'),'reference');
    assert.deepEqual(await list().locator('th[scope=row]').allTextContents(),['ORD-2099-21','ORD-2099-22']);
    await activate(page.locator('.shared-pagination a[rel=prev]'));assert.equal(await list().count(),20);
    assert.deepEqual(await page.locator('#order-page-size option').allTextContents(),['10','20','50','100']);
    await setSize('10');assert.equal(await list().count(),10);assert.equal(new URL(page.url()).searchParams.has('page'),false);assert.equal(await page.locator('#order-reference').inputValue(),'ORD-2099-');
    await activate(page.locator('.shared-pagination a[rel=next]'));assert.equal(await list().count(),10);assert.equal(new URL(page.url()).searchParams.get('per_page'),'10');
    await activate(page.locator('.shared-pagination a[rel=next]'));assert.equal(await list().count(),2);
    await setSize('50');assert.equal(await list().count(),22);assert.equal(new URL(page.url()).searchParams.has('page'),false);
    await setSize('100');assert.equal(await list().count(),22);
    await activate(page.getByRole('link',{name:lang==='it'?'Azzera filtri':'Reset filters',exact:true}));assert.equal(await list().count(),26);assert.equal(await page.locator('#order-page-size').inputValue(),'100');

    await activate(page.locator(`a.ui-table__link[href="/admin/orders/${records[0].id}"]`));
    await activate(page.locator('main .ui-page-header__back[href="/admin/orders"]'));
    assert.equal(new URL(page.url()).search,'');
    assert.equal(await page.locator('#order-page-size').inputValue(),'100','Return from order card remembers size');
    await page.goto(origin+'/admin/customers?per_page=50');
    await page.goto(origin+'/admin/catalog/products?per_page=50');
    await page.goto(origin+'/admin/orders');assert.equal(await page.locator('#order-page-size').inputValue(),'100','List preferences are independent');
    const preference=(await page.context().cookies()).find(c=>c.name==='talocart_admin_order_page_size');
    assert.ok(preference&&preference.value==='100'&&preference.httpOnly&&preference.sameSite==='Lax'&&preference.expires>Date.now()/1000+179*86400);
    await page.screenshot({path:path.join(output,`${name}-${lang}-order-page-size-persisted.png`),fullPage:true});
    await page.goto(origin+'/admin/orders?reference=ORD-2099-&per_page=10&page=2');
    await page.locator('#order-customer').fill('NO-SUCH-CUSTOMER');await submit();assert.equal(await list().count(),0);assert.equal(new URL(page.url()).searchParams.has('page'),false);
  }
};
