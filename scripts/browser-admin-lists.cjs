// Uses the demo suite's disposable schema and native navigation, including no-JS.
const assert = require('node:assert/strict');
const path = require('node:path');

module.exports = async ({ page, query, origin, name, output }) => {
  const original = (await query('SELECT palette,density FROM storefront_settings WHERE id=1')).rows[0];
  const viewport = page.viewportSize();
  await query(`INSERT INTO customer_groups(name_it,name_en)
    SELECT 'Gruppo elenco '||n,'List group '||n FROM generate_series(1,22) n`);
  await query(`INSERT INTO customers(email,password_hash,first_name,last_name,group_id)
    SELECT 'list-'||n||'@example.test','unused synthetic hash','Alex',
      'Un cognome molto lungo per verificare il ritorno a capo nei piccoli schermi',default_group_id
    FROM generate_series(1,22) n CROSS JOIN customer_group_configuration`);
  const check = async (route, selector) => {
    const response = await page.goto(origin + route);
    assert.equal(response.status(), 200, route);
    await page.waitForLoadState('load');
    const rows = page.locator(selector);
    assert.ok(await rows.count(), route + ': populated fixture');
    await page.mouse.move(0, 0);
    const isTable = await rows.first().evaluate(el => el.tagName === 'TR');
    const styles = await rows.evaluateAll(elements => {
      const probe = document.createElement('span');
      probe.style.backgroundColor = 'var(--white)'; document.body.append(probe);
      const white = getComputedStyle(probe).backgroundColor; probe.remove();
      return elements.flatMap((el, index) => {
        const table = el.tagName === 'TR';
        const surface = table ? el.closest('.ui-table-region') : el;
        return [...(table ? el.children : [el])].map(cell => {
          const style = getComputedStyle(cell);
          return { white, background: getComputedStyle(surface).backgroundColor,
            left: parseFloat(style.paddingLeft), right: parseFloat(style.paddingRight),
            border: parseFloat(style.borderBottomWidth), last: table && index === elements.length - 1 };
        });
      });
    });
    for (const style of styles) {
      assert.equal(style.background, style.white, route + ': white row surface');
      assert.ok(style.left >= (isTable ? 8 : 12) && style.right >= (isTable ? 8 : 12), route + ': component inset content');
      if (!style.last) assert.ok(style.border >= 1, route + ': thin separator');
      else assert.equal(style.border, 0, route + ': last table row uses the region border');
    }
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), route + ': no overflow');
    for (const pagination of await page.locator('.shared-pagination').all()) {
      assert.equal(await pagination.evaluate(el => getComputedStyle(el).backgroundColor), 'rgba(0, 0, 0, 0)', route + ': pagination keeps page surface');
    }
  };
  try {
    for (const locale of ['en', 'it']) {
      await query('UPDATE administrators SET locale=$1', [locale]);
      for (const [palette, density] of [['forest','comfortable'], ['ocean','compact'], ['clay','comfortable']]) {
        await query('UPDATE storefront_settings SET palette=$1,density=$2 WHERE id=1', [palette,density]);
        for (const [route, selector] of [
          ['/admin/customers?email=list-&per_page=20', '.customer-table tbody tr:has(th[scope=row])'], ['/admin/customer-groups', '.catalog-row'],
          ['/admin/catalog/options', '.catalog-row'], ['/admin/settings/payments', '.catalog-row'],
          ['/admin/users', '.user-row'], ['/admin/roles', '.user-row'], ['/admin/operations', '.operation-row'],
          ['/admin/catalog/brands', '.taxonomy-card'], ['/admin/catalog/categories', '.taxonomy-card'],
          ['/admin/components', '#admin-lists .catalog-row']
        ]) await check(route, selector);
        assert.equal(await page.locator('#admin-lists-title').innerText(), locale === 'it' ? 'Liste amministrative' : 'Administrative lists');
        await page.goto(origin + '/admin/customers?email=list-&per_page=20');
        assert.equal(await page.locator('html').getAttribute('lang'), locale);
        const next = page.locator('.shared-pagination a[rel=next]');
        await page.keyboard.press('Tab');
        await next.focus();
        assert.ok(await next.evaluate(el => el === document.activeElement && getComputedStyle(el).outlineStyle !== 'none'));
        await page.keyboard.press('Enter'); await page.waitForURL(url => url.pathname === '/admin/customers' && url.searchParams.get('page') === '2');
        assert.equal(new URL(page.url()).searchParams.get('email'), 'list-');
        assert.equal(new URL(page.url()).searchParams.get('per_page'), '20');
        assert.equal(await page.locator('.customer-table tbody tr:has(th[scope=row])').count(), 2);
        assert.equal(await page.locator('.shared-pagination [aria-disabled=true]').count(), 1);
        await page.locator('.shared-pagination a[rel=prev]').click(); await page.waitForURL(url => url.pathname === '/admin/customers' && url.searchParams.get('page') === '1');
        if (palette === 'ocean') {
          await page.goto(origin + '/admin/customers?email=list-&per_page=20&page=2');
          await page.screenshot({ path: path.join(output, `admin-lists-${name}-${locale}.png`), fullPage: true });
          await page.goto(origin + '/admin/customer-groups?page=2');
          await page.screenshot({ path: path.join(output, `admin-lists-groups-${name}-${locale}.png`), fullPage: true });
        }
      }
    }
  } finally {
    await query('UPDATE storefront_settings SET palette=$1,density=$2 WHERE id=1', [original.palette, original.density]);
    await query("UPDATE administrators SET locale='it'");
    await page.setViewportSize(viewport);
  }
};
