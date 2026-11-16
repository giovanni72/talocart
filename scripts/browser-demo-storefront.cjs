// Shared with every demo profile (desktop/mobile, JavaScript disabled and CLI seed).
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, query, origin, id, output, name, js }) => {
  await page.context().clearCookies({ name: 'talocart_catalog_page_size' });
  const publicGet = async path => {
    const r = await page.context().request.get(origin + path);
    assert.equal(r.status(), 200, path);
    assert.equal(r.headers()['x-robots-tag'], 'noindex, nofollow');
    return r;
  };
  const original = (await query('SELECT * FROM storefront_settings')).rows[0];
  assert.deepEqual([original.palette, original.typography, original.density], ['ocean', 'modern', 'compact'], 'New installations use the selected defaults');
  const navigate = async path => { await page.goto(origin + path); assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), path); };
  const save = async () => { await page.getByRole('button', { name: 'Salva impostazioni', exact: true }).click(); await page.waitForURL('**/admin/settings/storefront?saved=1'); await page.locator('#settings-saved').waitFor(); };
  await navigate('/admin/settings'); await page.locator('#settings-tab-storefront').click();
  assert.ok(await page.locator('#storefront').isVisible());
  // Homepage presentation is managed by the base banners area; the legacy flag is preserved.
  assert.equal(await page.getByLabel('Mostra la presentazione', {exact:true}).count(), 0);
  assert.equal(await page.locator('main a[href="/admin/banners"]').count(), 1);
  assert.ok(await page.getByLabel('Mostra i primi otto prodotti del catalogo', {exact:true}).evaluate(e => e.getBoundingClientRect().width <= 24), 'Homepage checkbox keeps its native width beside its label');
  await page.getByLabel('Indirizzo pubblico del negozio', { exact: true }).fill('https://user:secret@example.test');
  const invalid = page.waitForResponse(r => new URL(r.url()).pathname === '/admin/settings/save' && r.request().method() === 'POST');
  await page.getByRole('button', { name: 'Salva impostazioni', exact: true }).click(); assert.equal((await invalid).status(), 422);
  await page.getByRole('alert').waitFor(); assert.equal(await page.locator('#theme-public_origin').getAttribute('aria-invalid'), 'true');
  assert.deepEqual((await query('SELECT palette,typography,density FROM storefront_settings')).rows[0], { palette: original.palette, typography: original.typography, density: original.density }, 'An invalid save preserves every theme choice');
  for (const palette of ['forest', 'ocean', 'clay']) {
    for (const font of ['editorial', 'modern']) {
      for (const density of ['comfortable', 'compact']) {
        await navigate('/admin/settings/storefront');
        await page.getByLabel('Colori', { exact: true }).selectOption(palette);
        await page.getByLabel('Caratteri', { exact: true }).selectOption(font);
        await page.getByLabel('Densità', { exact: true }).selectOption(density);
        await page.getByLabel('Titolo della presentazione', { exact: true }).fill('Oggetti, storie e idee.');
        await page.getByLabel('Testo della presentazione', { exact: true }).fill('Una selezione da scoprire ogni giorno.');
        await page.getByLabel('Titolo SEO del negozio', { exact: true }).fill('Il nostro negozio & le sue idee');
        await page.getByLabel('Descrizione SEO del negozio', { exact: true }).fill('Prodotti scelti per te.');
        await page.getByLabel('Indirizzo pubblico del negozio', { exact: true }).fill('https://shop.example.test/');
        await save();
        if (palette==='forest' && font==='modern' && density==='compact') await page.screenshot({ path:path.join(output,`theme-settings-${name}.png`),fullPage:true });
        await navigate('/'); assert.equal(await page.title(), 'Il nostro negozio & le sue idee');
        assert.equal(await page.locator('h1').innerText(), 'Oggetti, storie e idee.');
        assert.equal(await page.locator('.home-featured .store-product').count(), 8);
        await require('./browser-product-card.cjs').check(page, true);
        assert.equal(await page.locator('.store-page').getAttribute('data-palette'), palette);
        assert.equal(await page.locator('.store-page').getAttribute('data-typography'), font);
        assert.equal(await page.locator('link[rel=canonical]').getAttribute('href'), 'https://shop.example.test/');
        assert.equal(await page.locator('meta[name=description]').getAttribute('content'), 'Prodotti scelti per te.');
        // Regression: the selected palette covers the whole page for both shell densities.
        if (font === 'modern') {
          const viewport = page.viewportSize();
          try {
            await page.setViewportSize({ width: 1920, height: 1080 });
            for (const route of ['/', '/catalog', '/products/tazza-atelier', '/status']) {
              await navigate(route);
              const colors = await page.evaluate(() => {
                const shell = document.querySelector('.store-shell');
                const bounds = shell.getBoundingClientRect();
                return { outer: getComputedStyle(document.body).backgroundColor, inner: getComputedStyle(shell).backgroundColor, left: bounds.left, right: bounds.right, viewport: innerWidth, padding: getComputedStyle(shell).paddingLeft };
              });
              assert.ok(Math.abs(parseFloat(colors.padding) - colors.viewport * .03) < 1, 'Storefront keeps 3% lateral padding');
              const expectedWidth = density === 'compact' ? 1440 : colors.viewport;
              assert.ok(Math.abs(colors.right - colors.left - expectedWidth) < 1, `${density} ${route}: expected shell width ${expectedWidth}`);
              assert.ok(Math.abs(colors.left - (colors.viewport - expectedWidth) / 2) < 1, `${density} ${route}: centered shell`);
              assert.equal(colors.outer, colors.inner, `${palette} ${route}: outer background matches the storefront`);
              const expected = { forest: 'rgb(246, 245, 239)', ocean: 'rgb(241, 245, 247)', clay: 'rgb(250, 243, 237)' };
              assert.equal(colors.outer, expected[palette], `${palette} ${route}: selected palette is applied`);
            }
            await navigate('/');
            await page.screenshot({ path: path.join(output, `theme-wide-${palette}-${name}.png`) });
            await navigate('/admin/settings/storefront');
            assert.equal(await page.locator('body').getAttribute('data-palette'), palette);
            assert.equal(await page.locator('body').evaluate(e => getComputedStyle(e).backgroundColor), { forest: 'rgb(246, 245, 239)', ocean: 'rgb(241, 245, 247)', clay: 'rgb(250, 243, 237)' }[palette], 'Admin inherits the saved palette');
          } finally { await page.setViewportSize(viewport); }
          await navigate('/');
        }
        const ratios = await page.evaluate(() => {
          const el = document.querySelector('.store-shell'), css = getComputedStyle(el);
          const luminance = text => {
            const hex = text.trim().replace('#', ''); const rgb = [0, 2, 4].map(i => parseInt(hex.slice(i, i + 2), 16) / 255).map(v => v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4);
            return .2126 * rgb[0] + .7152 * rgb[1] + .0722 * rgb[2];
          };
          const ratio = (a, b) => { const x = luminance(css.getPropertyValue(a)), y = luminance(css.getPropertyValue(b)); return (Math.max(x, y) + .05) / (Math.min(x, y) + .05); };
          return [ratio('--ink', '--paper'), ratio('--muted', '--paper'), ratio('--ink', '--white'), ratio('--muted', '--white'), ratio('--ink', '--accent')];
        });
        assert.ok(ratios.every(r => r >= 4.5), `${palette}: text contrast ${ratios}`);
        await page.locator('.skip-link').focus(); await page.keyboard.press('Enter'); assert.ok(await page.locator('#main').evaluate(e => e.getBoundingClientRect().top >= -1));
        const unlabeled = await page.evaluate(() => [...document.querySelectorAll('img:not([alt]), input:not([type=hidden]), select, textarea')].filter(e => e.tagName === 'IMG' || (!e.labels?.length && !e.getAttribute('aria-label'))).length);
        assert.equal(unlabeled, 0);
        if (font === 'modern' && density === 'compact') await page.screenshot({ path: path.join(output, `theme-${palette}-${name}.png`), fullPage: true });
      }
    }
  }
  await navigate('/admin/settings/storefront');
  await page.getByLabel('Mostra i primi otto prodotti del catalogo', { exact: true }).uncheck();
  await save(); await navigate('/'); assert.equal(await page.locator('.hero').count(), 0); assert.equal(await page.locator('.home-featured').count(), 0); assert.equal(await page.locator('h1').count(), 1);
  await navigate(`/admin/catalog/products/${id}`);
  await page.getByLabel('Titolo SEO', { exact: true }).fill('La tazza & le idee');
  await page.getByLabel('Descrizione SEO', { exact: true }).fill('Un prodotto <speciale>.');
  await page.getByRole('button', { name: 'Salva prodotto', exact: true }).click(); await page.waitForURL(`**/admin/catalog/products/${id}?saved=1**`);
  await navigate('/products/tazza-atelier'); assert.equal(await page.title(), 'La tazza & le idee'); assert.equal(await page.locator('meta[name=description]').getAttribute('content'), 'Un prodotto <speciale>.');
  assert.equal(await page.locator('link[rel=canonical]').getAttribute('href'), 'https://shop.example.test/products/tazza-atelier');
  const stock = (await query(`SELECT stock FROM catalog_products WHERE id=${id}`)).rows[0].stock;
  await query(`UPDATE catalog_products SET stock=0 WHERE id=${id}`);
  await navigate('/catalog/accessori');
  await page.getByLabel('Marchio', { exact: true }).selectOption('talocart-atelier');
  await page.getByLabel('Solo disponibili', { exact: true }).check();
  await page.getByRole('button', { name: 'Applica filtri', exact: true }).click(); await page.waitForURL(u => u.pathname === '/catalog/accessori' && u.searchParams.get('available') === 'true');
  assert.equal(new URL(page.url()).pathname, '/catalog/accessori'); assert.equal(new URL(page.url()).searchParams.get('brand'), 'talocart-atelier');
  assert.equal(await page.getByLabel('Solo disponibili', { exact: true }).isChecked(), true);
  assert.equal(await page.locator('.store-product a[href="/products/tazza-atelier"]').count(), 0);
  const count = Number((await query("WITH RECURSIVE subtree(id) AS (SELECT id FROM catalog_categories WHERE slug='accessori' UNION ALL SELECT c.id FROM catalog_categories c JOIN subtree parent ON c.parent_id=parent.id) SELECT count(*) FROM catalog_visible_products p JOIN catalog_products source ON source.id=p.id JOIN catalog_brands b ON b.id=p.brand_id WHERE p.category_id IN (SELECT id FROM subtree) AND b.slug='talocart-atelier' AND p.stock>=((source.minimum_quantity+source.packs-1)/source.packs)*source.packs")).rows[0].count);
  assert.equal(await page.locator('.store-product').count(), Math.min(21, count));
  await page.getByRole('link', { name: 'Rimuovi filtri', exact: true }).click(); await page.waitForURL('**/catalog/accessori');
  await page.goto(origin + '/catalog');
  await page.getByRole('navigation', { name: 'Pagine catalogo' }).getByRole('link', { name: 'Successivo', exact: true }).click(); await page.waitForURL(u => u.pathname === '/catalog' && u.searchParams.get('page') === '2'); assert.equal(await page.locator('.store-product').count(), 6);
  await query(`UPDATE catalog_products SET stock=${stock} WHERE id=${id}`);
  for (const route of ['/sitemap.xml', '/sitemap-catalog.xml', '/sitemap-products.xml?page=1']) {
    const r = await publicGet(route); assert.equal(r.headers()['content-type'], 'application/xml; charset=utf-8');
    const xml = await r.text(); const data = await page.evaluate(xml => { const d = new DOMParser().parseFromString(xml, 'application/xml'); return { errors: d.querySelectorAll('parsererror').length, urls: [...d.querySelectorAll('loc')].map(e => e.textContent) }; }, xml);
    assert.equal(data.errors, 0); assert.ok(data.urls.every(u => u.startsWith('https://shop.example.test/')));
    if (route === '/sitemap-products.xml?page=1') { assert.equal(data.urls.length, 54); const italian = data.urls.filter(url => new URL(url).pathname.startsWith('/products/')); assert.equal(italian.length, 27); assert.equal(new Set(data.urls).size, 54); for (const url of italian) assert.ok(data.urls.includes(url.replace('/products/', '/en/products/')), 'Every public product has both language URLs'); assert.ok(data.urls.includes('https://shop.example.test/products/tshirt-atelier')); assert.ok(!data.urls.some(u => u.includes('prodotto-bozza'))); }
  }
  const robots = await (await publicGet('/robots.txt')).text(); assert.ok(robots.includes('Disallow:\n')); assert.ok(!robots.includes('Sitemap:'));
  // A direct link may select an active brand beyond the first 1000 choices.
  await query("INSERT INTO catalog_brands(name,slug) SELECT 'Audit brand '||lpad(n::text,4,'0'),'audit-brand-'||n FROM generate_series(1,1001) n");
  try {
    await navigate('/catalog?brand=audit-brand-1001');
    await page.getByLabel('Cerca un prodotto', { exact: true }).fill('tazza');
    await page.getByRole('button', { name: 'Cerca', exact: true }).click();
    await page.waitForURL(u => u.pathname === '/search/tazza');
    assert.equal(new URL(page.url()).searchParams.get('brand'), 'audit-brand-1001', 'Searching must preserve a selected brand beyond the bounded choices');
    assert.equal(await page.getByLabel('Marchio', { exact: true }).inputValue(), 'audit-brand-1001');
  } finally {
    await query("DELETE FROM catalog_brands WHERE slug LIKE 'audit-brand-%'");
  }
  // Restore synthetic presentation settings using the same user-visible form.
  await navigate('/admin/settings/storefront');
  for (const key of ['palette', 'typography', 'density']) await page.locator(`#theme-${key}`).selectOption(original[key]);
  for (const key of ['heading', 'introduction', 'seo_title', 'seo_description', 'public_origin']) await page.locator(`#theme-${key}`).fill(original[key]);
  await page.locator('input[name=show_featured]').setChecked(original.show_featured);
  await save(); assert.equal((await query('SELECT show_hero FROM storefront_settings')).rows[0].show_hero, original.show_hero, 'Theme saves preserve the legacy presentation flag'); await navigate('/catalog');
};
