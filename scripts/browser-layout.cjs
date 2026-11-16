// Shared layout checks run against the demo suite's disposable database and media.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, origin, viewport, output, name, query, js }) => {
  const mug = (await query("SELECT id FROM catalog_products WHERE model='MUG'")).rows[0].id;
  const category = (await query("SELECT id FROM catalog_categories ORDER BY id LIMIT 1")).rows[0].id;
  const brand = (await query("SELECT id FROM catalog_brands ORDER BY id LIMIT 1")).rows[0].id;
  const role = (await query("SELECT key FROM admin_roles ORDER BY key LIMIT 1")).rows[0].key;
  const user = (await query("SELECT id FROM administrators ORDER BY id LIMIT 1")).rows[0].id;
  const option = (await query("SELECT id FROM catalog_options ORDER BY id LIMIT 1")).rows[0].id;
  const variant = (await query("SELECT id, product_id FROM catalog_variants ORDER BY id LIMIT 1")).rows[0];
  const rate = (await query("SELECT id FROM tax_rates ORDER BY id LIMIT 1")).rows[0].id;
  const tax = (await query("SELECT id FROM tax_classes ORDER BY id LIMIT 1")).rows[0].id;
  const routes = ['/', '/catalog', '/products/tazza-atelier', '/status', '/admin', '/admin/components',
    '/admin/catalog/products', '/admin/catalog/products/new', `/admin/catalog/products/${mug}`,
    '/admin/catalog/categories', '/admin/catalog/categories/new', `/admin/catalog/categories/${category}`,
    '/admin/catalog/brands', '/admin/catalog/brands/new', `/admin/catalog/brands/${brand}`,
    '/admin/catalog/options', '/admin/catalog/options/new', '/admin/settings', '/admin/settings/appearance',
    '/admin/settings/storefront', '/admin/settings/taxes', '/admin/settings/taxes/rates',
    '/admin/settings/taxes/rates/new', '/admin/settings/taxes/classes', '/admin/settings/taxes/classes/new',
    '/admin/account', '/admin/users', '/admin/users/new', `/admin/users/${user}`,
    '/admin/roles', '/admin/roles/new', `/admin/roles/${role}`, '/admin/modules', '/admin/access',
    `/admin/catalog/options/${option}`, `/admin/catalog/products/${variant.product_id}/variants`,
    `/admin/catalog/products/${variant.product_id}/variants/new`,
    `/admin/catalog/products/${variant.product_id}/variants/${variant.id}`,
    `/admin/settings/taxes/rates/${rate}`, `/admin/settings/taxes/classes/${tax}`];
  const headers = ['ID', 'Immagine', 'Tipo', 'Nome', 'Modello', 'Categoria', 'Marchio', 'Prezzo', 'Varianti', 'Scorte', 'Stato', 'Ultima modifica', 'Azioni'];
  try {
    for (const width of [2560, 1920, 1440, 1280, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      for (const route of routes) {
        const response = await page.goto(origin + route);
        assert.equal(response.status(), 200, `${name} ${width} ${route}`);
        const metrics = await page.evaluate(() => {
          const main = document.querySelector('.admin-main');
          const store = document.querySelector('.store-shell');
          return { overflow: document.documentElement.scrollWidth > innerWidth,
            main: main?.getBoundingClientRect().width,
            workspace: document.querySelector('.admin-workspace')?.getBoundingClientRect().width,
            store: store?.getBoundingClientRect().width };
        });
        assert.equal(metrics.overflow, false, `No page overflow: ${name} ${width} ${route}`);
        await require('./ui-contract.cjs')(page);
        if (metrics.main) assert.ok(Math.abs(metrics.main - metrics.workspace) < 1, `Admin uses workspace width: ${width} ${route}`);
        if (metrics.store) assert.ok(Math.abs(metrics.store - Math.min(width, 1440)) < 1, `Compact storefront has a responsive maximum width: ${width} ${route}`);
        if (route === '/admin/catalog/products' || route === `/admin/catalog/products/${variant.product_id}/variants`) {
          const offsets = await page.locator('.ui-table .product-row-image').evaluateAll(images => images.map(image => {
            const rect = image.getBoundingClientRect(), cell = image.closest('td').getBoundingClientRect();
            return Math.abs((rect.left + rect.right - cell.left - cell.right) / 2);
          }));
          assert.ok(offsets.length > 0 && offsets.every(offset => offset < 1), 'Product and variant images are horizontally centered in their cells');
        }
        if (route === `/admin/catalog/products/${variant.product_id}/variants`) {
          assert.equal(await page.locator('.product-pagination-toolbar .catalog-result-count').innerText(), '4 varianti');
          assert.ok(await page.locator('.catalog-result-count').evaluate(count => {
            const toolbar = count.closest('.product-pagination-toolbar'), rect = count.getBoundingClientRect();
            const table = document.querySelector('.ui-table-region').getBoundingClientRect();
            const selector = toolbar.querySelector('select').getBoundingClientRect();
            return Math.abs(table.left-rect.left)<1 && (innerWidth<1280 || Math.abs((rect.top+rect.bottom-selector.top-selector.bottom)/2)<1);
          }), 'Variant total is at the left of the quantity toolbar and aligned with the selector on desktop');
          if (width >= 1280) assert.ok(await page.locator('.ui-page-header h1').evaluate(title => document.querySelector('.variant-table').getBoundingClientRect().top-title.getBoundingClientRect().bottom<=120), 'Variant title and table have compact spacing');
          assert.deepEqual(await page.locator('#variant-page-size option').allTextContents(), ['10','20','50','100']);
          assert.equal(await page.locator('#variant-page-size').inputValue(), '20');
          assert.ok(await page.locator('.product-pagination-toolbar').evaluate(toolbar => toolbar.getBoundingClientRect().bottom<=document.querySelector('.variant-table').getBoundingClientRect().top), 'Variant size selector is above the table');
          assert.deepEqual(await page.locator('.variant-table thead th').allTextContents(), ['Immagine', 'Combinazione', 'SKU', 'Prezzo', 'Scorte', 'Stato', 'Azioni']);
          assert.equal(await page.locator('.variant-table tbody tr').count(), 4);
          assert.equal(await page.locator('.variant-table thead tr').count(), 2);
          assert.equal(await page.locator('.variant-table .ui-table__filters td').count(), 7);
          for (const [column, content] of [[6, '.catalog-status'], [7, '.catalog-edit']]) {
            const offsets = await page.locator(`.variant-table thead th:nth-child(${column}), .variant-table tbody td:nth-child(${column}) ${content}`).evaluateAll(elements => elements.map(element => {
              const cell = element.closest('th, td').getBoundingClientRect();
              let rect = element.getBoundingClientRect();
              if (element.tagName === 'TH') {
                const range = document.createRange(); range.selectNodeContents(element); rect = range.getBoundingClientRect();
              }
              return Math.abs((rect.left + rect.right - cell.left - cell.right) / 2);
            }));
            assert.equal(offsets.length, 5, 'Variant heading and all four row contents are checked');
            assert.ok(offsets.every(offset => offset < 1), `Variant status/actions are horizontally centered: ${name} ${width} column ${column}`);
          }
          for (const [column, field] of [[2, 'combination'], [3, 'sku'], [6, 'status']]) {
            const control = page.locator(`.variant-table .ui-table__filters td:nth-child(${column}) [name="${field}"]`);
            assert.equal(await control.count(), 1, `Variant filter ${field} is under its heading`);
            assert.ok(await control.evaluate(e => e.labels.length > 0), `Variant filter ${field} has a label`);
          }
          assert.ok(await page.locator('.variant-table .catalog-edit').evaluateAll(buttons => buttons.length === 4 && buttons.every(button => button.classList.contains('ui-button--compact') && getComputedStyle(button).minHeight === '36px' && getComputedStyle(button).paddingTop === '4px' && getComputedStyle(button).paddingBottom === '4px')), 'Variant edit uses the shared compact button size');
          if (width >= 1280) assert.ok(await page.locator('.ui-table-region').evaluate(e => e.scrollWidth <= e.clientWidth + 1));
          if ([1920, 390].includes(width)) await page.screenshot({ path: path.join(output, `variant-table-${name}-${width}.png`) });
        }
        if (route === '/admin/catalog/products') {
          assert.deepEqual(await page.locator('.product-table thead th .ui-table__sort-label').allTextContents(), headers);
          if (width >= 1280) assert.ok(await page.locator('.ui-table__sort-label').getByText('Immagine', {exact:true}).evaluate(e => e.getBoundingClientRect().height <= parseFloat(getComputedStyle(e).lineHeight) + 1), 'Image heading and sort indicator remain on one line on desktop');
          assert.equal(await page.locator('.product-table thead tr').count(), 2);
          assert.ok(await page.locator('.product-table .catalog-status').evaluateAll(badges => badges.every(badge => {
            const style = getComputedStyle(badge), rect = badge.getBoundingClientRect();
            return rect.height <= parseFloat(style.lineHeight) + parseFloat(style.paddingTop) + parseFloat(style.paddingBottom) + 1;
          })), `Status labels remain readable on one line: ${name} ${width}`);
          for (const [column, content] of [[12, '.catalog-status'], [14, '.catalog-edit']]) {
            const offsets = await page.locator(`.product-table thead tr:first-child th:nth-child(${column}) .ui-table__sort-label, .product-table tbody td:nth-child(${column}) ${content}`).evaluateAll(elements => elements.map(element => {
              const rect = element.getBoundingClientRect(), cell = element.closest('th, td').getBoundingClientRect();
              return Math.abs((rect.left + rect.right - cell.left - cell.right) / 2);
            }));
            assert.equal(offsets.length, 21, 'Heading and all twenty row contents are checked');
            assert.ok(offsets.every(offset => offset < 1), `Status and actions are horizontally centered: ${name} ${width} column ${column}: ${JSON.stringify(offsets)}`);
          }
          assert.ok(await page.locator('.product-table .catalog-edit').evaluateAll(buttons => buttons.every(button => button.classList.contains('ui-button--compact') && getComputedStyle(button).minHeight === '36px' && getComputedStyle(button).paddingTop === '4px' && getComputedStyle(button).paddingBottom === '4px')), 'Edit uses the shared compact button size');
          assert.ok(await page.locator('.product-table .product-row-image').evaluateAll(images => images.length > 0 && images.every(image => getComputedStyle(image).height === '56px' && getComputedStyle(image).objectFit === 'contain')), 'Product thumbnails are taller and preserve the complete image');
          const variantCenter = await page.locator('.product-table thead th:nth-child(10) .ui-table__sort-label').evaluate(e => {
            const label=e.getBoundingClientRect(), cell=e.closest('th').getBoundingClientRect();
            return Math.abs((label.left+label.right-cell.left-cell.right)/2);
          });
          assert.ok(variantCenter < 1, 'Variants heading label is horizontally centered in its cell');
          const indicatorGeometry = await page.locator('.product-table thead th:nth-child(10) .ui-table__sort').evaluate(e => {
            const label=e.querySelector('.ui-table__sort-label').getBoundingClientRect(), indicator=e.querySelector('.ui-table__sort-indicator').getBoundingClientRect(), cell=e.closest('th').getBoundingClientRect();
            return {labelRight:label.right,indicatorLeft:indicator.left,indicatorRight:indicator.right,cellRight:cell.right,cellWidth:cell.width};
          });
          assert.ok(indicatorGeometry.indicatorLeft >= indicatorGeometry.labelRight && indicatorGeometry.indicatorRight <= indicatorGeometry.cellRight, `Variant indicator remains visible without covering the centered label: ${name} ${width} ${JSON.stringify(indicatorGeometry)}`);
          const filters = page.locator('.ui-table__filters');
          assert.equal(await filters.locator('td').count(), headers.length + 1);
          for (const [column, field] of [[3, 'type'], [4, 'q'], [5, 'model'], [6, 'category'], [7, 'brand'], [11, 'status']]) {
            const control = filters.locator(`td:nth-child(${column + 1}) [name="${field}"]`);
            assert.equal(await control.count(), 1, `Filter ${field} in its column`);
            assert.ok(await control.evaluate(e => e.labels.length > 0), `Accessible label for ${field}`);
            assert.ok(await control.evaluate(e => { const r=e.getBoundingClientRect(), c=e.closest('td').getBoundingClientRect(); return r.left >= c.left && r.right <= c.right; }), `Control ${field} fits its cell`);
          }
          for (const field of ['category','brand']) assert.equal(await filters.locator(`[name="${field}"]`).evaluate(e => e.tagName), 'SELECT');
          const actions = filters.locator('td:last-child');
          const filterButton = await actions.getByRole('button', {name:'Filtra',exact:true}).boundingBox();
          const resetButton = await actions.getByRole('link', {name:'Azzera filtri',exact:true}).boundingBox();
          assert.ok(Math.abs(resetButton.y - filterButton.y) < 1, 'Filter and reset share one row');
          assert.ok(Math.abs(resetButton.width - resetButton.height) < 1 && resetButton.width >= 36, 'Reset is a square compact control');
          assert.ok(filterButton.width > resetButton.width, 'Filter takes more width than reset');
          assert.ok(resetButton.x >= filterButton.x + filterButton.width, 'Reset follows filter');
          assert.equal(await actions.getByRole('link', {name:'Azzera filtri',exact:true}).innerText(), '');
          assert.equal(await actions.getByRole('link', {name:'Azzera filtri',exact:true}).locator('svg').getAttribute('data-ui-icon'), 'x');
          const actionStyle = await actions.locator('.product-filter-actions').evaluate(e => {
            const rect=e.getBoundingClientRect(), first=e.firstElementChild.getBoundingClientRect(), last=e.lastElementChild.getBoundingClientRect();
            const style=getComputedStyle(e.lastElementChild);
            const padding = parseFloat(getComputedStyle(e).paddingLeft);
            return {fill:Math.abs(first.left-rect.left-padding)<1 && Math.abs(last.right-rect.right+padding)<1, background:style.backgroundColor};
          });
          assert.ok(actionStyle.fill, 'Actions use all available width');
          assert.equal(actionStyle.background, await page.locator('body').evaluate(e => { const hex=getComputedStyle(e).getPropertyValue('--white').trim().slice(1);return `rgb(${[0,2,4].map(i=>parseInt(hex.slice(i,i+2),16)).join(', ')})`; }), 'Reset uses the shared secondary button');
          assert.equal(await page.locator('.product-table tbody tr').count(), 20);
          const placeholders = await page.locator('.product-table [aria-label="Immagine non disponibile"]').evaluateAll(elements => elements.map(e => {
            const range = document.createRange(); range.selectNodeContents(e);
            const text = range.getBoundingClientRect(); const cell = e.closest('td').getBoundingClientRect();
            return Math.abs((text.left + text.right) / 2 - (cell.left + cell.right) / 2);
          }));
          assert.ok(placeholders.length > 0);
          assert.ok(placeholders.every(offset => offset < 1), 'Missing-image dash is horizontally centered in its cell');
          // The native table remains readable and keyboard-scrollable on small screens.
          const box = page.locator('.ui-table-region');
          assert.equal(await box.getAttribute('tabindex'), '0');
          if (width >= 1600) assert.ok(await box.evaluate(e => e.scrollWidth <= e.clientWidth + 1), `Desktop table fits without horizontal scrolling (${name}, ${width})`);
          else {
            assert.ok(await box.evaluate(e => e.scrollWidth > e.clientWidth), 'Small screens retain a readable table');
            await box.focus();
            assert.equal(await box.evaluate(e => e === document.activeElement), true);
            await page.keyboard.press('ArrowRight');
            // Poll from Node: page requestAnimationFrame callbacks do not run with JS disabled.
            let scrolled = false;
            for (let attempt = 0; attempt < 20; attempt++) {
              scrolled = await box.evaluate(e => e.scrollLeft > 0);
              if (scrolled) break;
              await new Promise(resolve => setTimeout(resolve, 50));
            }
            assert.ok(scrolled, `Keyboard scroll works: ${name} ${width}`);
          }
          if ([1920, 1280, 390].includes(width)) await page.screenshot({ path: path.join(output, `product-table-${name}-${width}.png`) });
        }
      }
    }
    await page.setViewportSize(viewport);
    await require('./browser-variant-filters.cjs')({page, origin, name, output, query, js});
    await require('./browser-product-selection.cjs')({page, origin, name});
    await require('./browser-product-pagination.cjs')({page, origin, name, output});
    await require('./browser-product-sorting.cjs')({page, origin, output, name, query});
    await page.goto(origin + '/admin/catalog/products?q=SHIRT');
    const countLink = page.getByRole('link', { name: 'Apri 4 varianti di T-shirt Atelier', exact: true });
    assert.equal(await countLink.innerText(), '4');
    const square = await countLink.evaluate(e => { const r = e.getBoundingClientRect(); return {width:r.width,height:r.height}; });
    assert.equal(square.width, square.height); assert.ok(square.width >= 32);
    await countLink.focus();
    await page.screenshot({ path: path.join(output, `variant-count-${name}.png`), fullPage:true });
    await page.keyboard.press('Enter');
    await page.waitForURL(`**/admin/catalog/products/${variant.product_id}/variants`);
    assert.equal(await page.locator('.variant-table tbody tr').count(), 4);
    assert.equal(await page.getByText('Disattivata', {exact:true}).count(), 1);
    assert.equal(await page.locator('.variant-table img[alt="Immagine del prodotto"]').count(), 2);
    await page.getByRole('link', { name: 'Modifica SHIRT-BLUE-M', exact:true }).click();
    assert.equal(await page.getByLabel('SKU variante', {exact:true}).inputValue(), 'SHIRT-BLUE-M');
    await page.goto(origin + '/admin/catalog/products?q=EMPTY');
    await page.getByRole('link', { name: 'Apri 0 varianti di Varianti da completare', exact:true }).click();
    await page.getByRole('heading', {name:'La prima combinazione.',exact:true}).waitFor();
    const original = (await query('SELECT name, model FROM catalog_products WHERE id=$1', [mug])).rows[0];
    try {
      await query('WITH u AS (UPDATE catalog_products SET name=$1, model=$2 WHERE id=$3 RETURNING id,name) UPDATE catalog_products_translations t SET name=u.name FROM u WHERE t.entity_id=u.id AND t.locale=(SELECT primary_locale FROM storefront_settings WHERE id=1)', ['A'.repeat(120), 'LONG'.repeat(16), mug]);
      for (const width of [1920, 1280]) {
        await page.setViewportSize({ width, height: 1000 });
        await page.goto(origin + '/admin/catalog/products');
        assert.ok(await page.locator('.ui-table-region').evaluate(e => e.scrollWidth <= Math.max(1280, e.clientWidth) + 1), 'Long unbroken names/SKUs do not enlarge the readable table');
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      }
    } finally { await query('WITH u AS (UPDATE catalog_products SET name=$1, model=$2 WHERE id=$3 RETURNING id,name) UPDATE catalog_products_translations t SET name=u.name FROM u WHERE t.entity_id=u.id AND t.locale=(SELECT primary_locale FROM storefront_settings WHERE id=1)', [original.name, original.model, mug]); }
    await page.setViewportSize(viewport);
    await page.goto(origin + '/admin/catalog/products');
    const mugFilters = (await query('SELECT p.name, p.category_id, p.brand_id FROM catalog_products p JOIN catalog_categories c ON c.id=p.category_id JOIN catalog_brands b ON b.id=p.brand_id WHERE p.id=$1', [mug])).rows[0];
    await page.getByLabel('Filtra per tipo', {exact:true}).selectOption('simple');
    await page.getByLabel('Cerca per nome, modello o slug').fill(mugFilters.name);
    await page.getByLabel('Filtra per modello', {exact:true}).fill('MUG');
    await page.getByLabel('Filtra per categoria', {exact:true}).selectOption(String(mugFilters.category_id));
    await page.getByLabel('Filtra per marchio', {exact:true}).selectOption(String(mugFilters.brand_id));
    await page.locator('.ui-table__filters').getByLabel('Stato', {exact:true}).selectOption('published');
    await page.getByRole('button', {name:'Filtra',exact:true}).click();
    assert.equal(await page.locator('.product-table tbody th[scope=row]').count(), 1);
    assert.equal(await page.getByLabel('Filtra per categoria', {exact:true}).inputValue(), String(mugFilters.category_id));
    assert.equal(await page.getByLabel('Filtra per marchio', {exact:true}).inputValue(), String(mugFilters.brand_id));
    await page.getByLabel('Filtra per tipo', {exact:true}).selectOption('variants');
    await page.getByRole('button', {name:'Filtra',exact:true}).click();
    await page.getByRole('heading', {name:'Nessun risultato.',exact:true}).waitFor();
    assert.equal(await page.locator('.product-table thead tr').count(), 2);
    await page.getByRole('link', {name:'Azzera filtri',exact:true}).focus();
    await page.keyboard.press('Enter');
    await page.waitForURL(url => url.pathname === '/admin/catalog/products' && url.search === '');
    assert.equal(new URL(page.url()).search, '');
    for (const field of ['q','type','model','category','brand','status']) assert.equal(await page.locator(`.ui-table__filters [name="${field}"]`).inputValue(), '');
    await page.getByLabel('Filtra per tipo', {exact:true}).selectOption('simple');
    await page.locator('.ui-table__filters').getByLabel('Stato', {exact:true}).selectOption('published');
    await page.getByRole('button', {name:'Filtra',exact:true}).click();
    await page.getByRole('link', {name:'Successivo',exact:true}).click();
    assert.equal(new URL(page.url()).searchParams.get('page'), '2');
    assert.equal(await page.getByLabel('Filtra per tipo', {exact:true}).inputValue(), 'simple');
    assert.equal(await page.locator('.ui-table__filters').getByLabel('Stato', {exact:true}).inputValue(), 'published');
    await page.getByLabel('Cerca per nome, modello o slug').fill('BREAKFAST-MUG');
    await page.getByLabel('Cerca per nome, modello o slug').press('Enter');
    assert.equal(new URL(page.url()).searchParams.get('page'), null, 'A new filter starts on page one');
    assert.equal(await page.locator('.product-table tbody th[scope=row]').count(), 1);
    await page.getByRole('link', {name:'Azzera filtri',exact:true}).click();
    await page.getByLabel('Cerca per nome, modello o slug').fill('BREAKFAST-MUG');
    await page.getByRole('button', { name: 'Filtra', exact: true }).click();
    assert.equal(await page.locator('.product-table tbody tr').count(), 1);
    await page.getByLabel('Cerca per nome, modello o slug').fill('no-such-product');
    await page.getByRole('button', { name: 'Filtra', exact: true }).click();
    assert.equal(await page.getByRole('heading', { name: 'Nessun risultato.' }).count(), 1);
  } finally { await page.setViewportSize(viewport); }
};
