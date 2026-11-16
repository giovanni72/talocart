// Customer list interactions, called inside the isolated customer-admin fixture.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page,db,origin,locale,js,output,suffix,group}) => {
  const emails=[];
  try {
    for(let n=0;n<23;n++) {
      const email=`table-${suffix}-${String(n).padStart(2,'0')}@example.test`;emails.push(email);
      await db.query("INSERT INTO customers(email,password_hash,first_name,last_name,customer_type,active,group_id) VALUES($1,'synthetic-unused-hash',$2,'Table','business',false,$3)",[email,`Customer ${String(n).padStart(2,'0')}`,group]);
    }
    const activate=async target=>{await target.focus();await Promise.all([page.waitForNavigation({waitUntil:'load'}),page.keyboard.press('Enter')]);};
    const submit=()=>activate(page.locator('.customer-table-search button[type=submit]'));
    const rows=()=>page.locator('.customer-table tbody tr:has(th[scope=row])');
    const header=key=>page.locator('th[aria-sort]').filter({has:page.locator(`a[href*="sort=${key}&"]`)});
    await page.goto(origin+'/admin/customers');
    assert.equal(await page.locator('th[aria-sort]').count(),5);
    assert.equal(await page.locator('.ui-table__filters td').count(),6);
    await page.locator('#customer-name').fill('CUSTOMER');
    await page.locator('#customer-email').fill(`table-${suffix}-`);
    await page.locator('#customer-group').selectOption(String(group));
    await page.locator('#customer-kind').selectOption('business');
    await page.locator('#customer-state').selectOption('inactive');
    await submit();assert.equal(await rows().count(),20);
    await page.locator('#customer-page-size').selectOption('10');
    if(js)await page.waitForURL('**/admin/customers?**per_page=10');
    else await activate(page.locator('.product-page-size button[type=submit]'));
    assert.equal(await rows().count(),10);
    await activate(header('name').getByRole('link'));assert.equal(await header('name').getAttribute('aria-sort'),'descending');
    assert.equal((await rows().first().locator('th').innerText()).trim(),'Customer 22 Table');
    await activate(header('name').getByRole('link'));assert.equal(await header('name').getAttribute('aria-sort'),'ascending');
    for(const key of ['email','group','kind','state']) {
      await activate(header(key).getByRole('link'));assert.equal(await header(key).getAttribute('aria-sort'),'ascending');
      await activate(header(key).getByRole('link'));assert.equal(await header(key).getAttribute('aria-sort'),'descending');
      assert.equal(await page.locator('#customer-email').inputValue(),`table-${suffix}-`);
    }
    await activate(header('name').getByRole('link'));
    await activate(page.locator('.shared-pagination a[rel=next]'));
    assert.equal(await rows().count(),10);assert.equal((await rows().first().locator('th').innerText()).trim(),'Customer 10 Table');
    assert.equal(await page.locator('#customer-group').inputValue(),String(group));
    await activate(page.locator('.shared-pagination a[rel=next]'));
    assert.equal(await rows().count(),3);assert.equal((await rows().first().locator('th').innerText()).trim(),'Customer 20 Table');
    await activate(page.locator('.shared-pagination a[rel=prev]'));assert.equal(await rows().count(),10);
    await page.locator('#customer-name').focus();await page.keyboard.press('Tab');assert.equal(await page.locator(':focus').getAttribute('id'),'customer-email');
    assert.notEqual(await page.locator(':focus').evaluate(e=>getComputedStyle(e).outlineStyle),'none');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    await page.screenshot({path:path.join(output,suffix+'-customer-table.png'),fullPage:true});
    const region=page.locator('.customer-table-search .ui-table-region');
    await region.focus();await page.keyboard.press('ArrowRight');
    if((await region.evaluate(e=>e.scrollWidth>e.clientWidth))) {
      await page.waitForFunction(()=>document.querySelector('.customer-table-search .ui-table-region').scrollLeft>0);
      assert.ok(await region.evaluate(e=>e.scrollLeft>0));
    }
    await page.locator('#customer-state').selectOption('active');await submit();assert.equal(await rows().count(),0);
    assert.ok((await page.locator('.ui-empty').innerText()).includes(locale==='it'?'Nessun risultato':'No results'));
    await activate(page.getByRole('link',{name:locale==='it'?'Azzera filtri':'Reset filters',exact:true}));
    assert.equal(await page.locator('#customer-name').inputValue(),'');assert.equal(await page.locator('#customer-state').inputValue(),'');
    assert.equal(await page.locator('#customer-page-size').inputValue(),'10');
    const response=await page.goto(origin+'/admin/customers?sort=invalid&email='+encodeURIComponent(`table-${suffix}-`));
    assert.equal(response.status(),422);assert.equal(await page.locator('#customer-filter-error').count(),1);
    assert.equal(await page.locator('#customer-email').inputValue(),`table-${suffix}-`);
    assert.equal(await page.locator('.shared-pagination').count(),0);
    await activate(page.getByRole('link',{name:locale==='it'?'Azzera filtri':'Reset filters',exact:true}));
    await page.locator('.customer-table tbody a.ui-table__link').first().focus();
    await Promise.all([page.waitForURL(/\/admin\/customers\/\d+$/),page.keyboard.press('Enter')]);
    assert.equal(await page.locator('#customer-profile-form').count(),1);
    await activate(page.locator('main .ui-page-header__back'));
    assert.equal(new URL(page.url()).search,'');
    assert.equal(await page.locator('#customer-page-size').inputValue(),'10','Return from customer card remembers size');
    await page.goto(origin+'/admin/orders?per_page=50');
    await page.goto(origin+'/admin/catalog/products?per_page=100');
    await page.goto(origin+'/admin/orders');assert.equal(await page.locator('#order-page-size').inputValue(),'50');
    await page.goto(origin+'/admin/customers');assert.equal(await page.locator('#customer-page-size').inputValue(),'10','List preferences are independent');
    const preference=(await page.context().cookies()).find(c=>c.name==='talocart_admin_customer_page_size');
    assert.ok(preference&&preference.value==='10'&&preference.httpOnly&&preference.sameSite==='Lax'&&preference.expires>Date.now()/1000+179*86400);
    await page.screenshot({path:path.join(output,suffix+'-customer-page-size-persisted.png'),fullPage:true});

  } finally {
    await db.query('DELETE FROM customers WHERE email=ANY($1::text[])',[emails]);
    await page.goto(origin+'/admin/customers');
  }
};
