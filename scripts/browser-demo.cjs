const presentation = require('./theme-presentation-helper.cjs');
// Development-only: every scenario uses an isolated PostgreSQL schema and media root.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes, createHash } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { Client } = require('pg');
const { chromium } = require('playwright');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test database required');
  const owner = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL });
  await owner.connect();
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {});
  const output = path.resolve('.local/verification/demo-seeders'); fs.mkdirSync(output, { recursive: true });
  const results = [];
  try {
    for (const [name, viewport, js, withDemo] of [['desktop', { width: 1440, height: 1050 }, true, true], ['mobile', { width: 390, height: 844 }, true, true], ['no-js', { width: 1440, height: 1050 }, false, true], ['mobile-no-js', { width: 390, height: 844 }, false, true], ['cli', { width: 1440, height: 1050 }, true, false]].filter(([name]) => (!(process.env.TALOCART_DEMO_VARIANT_COMMERCE_ONLY || process.env.TALOCART_DEMO_IMAGES_ONLY) || name !== 'cli') && (!process.env.TALOCART_DEMO_TEST_CASE || name === process.env.TALOCART_DEMO_TEST_CASE))) {
      const schema = `talocart_demo_browser_${process.pid}_${Date.now()}`;
      const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-demo-browser-'));
      const port = Number(process.env.TALOCART_DEMO_TEST_PORT || 3102); const origin = `http://127.0.0.1:${port}`;
      const url = new URL(process.env.TALOCART_TEST_DATABASE_URL); url.searchParams.set('options', `-csearch_path=${schema}`);
      const config = path.join(folder, 'talocart.toml'); fs.writeFileSync(config, `[server]\nlisten="127.0.0.1:${port}"\n`, { mode: 0o600 });
      const media = path.join(folder, 'media'); const env = { ...process.env, TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: `127.0.0.1:${port}`, TALOCART_MEDIA_ROOT: media, TALOCART_LOG: 'warn', TALOCART_MEDIA_DISCOVERY_INTERVAL_SECONDS: '1' };
      const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
      const cli = (...args) => { const r = spawnSync(binary, ['--config', config, ...args], { env, encoding: 'utf8' }); assert.equal(r.status, 0, `CLI ${args[0]} failed (private output omitted)`); return r.stdout; };
      const query = async (sql, params) => (await owner.query(`SET search_path TO ${schema}`), await owner.query(sql, params));
      let server; const context = await browser.newContext({ viewport, javaScriptEnabled: js });
      context.on('page', p => p.on('dialog', d => d.accept()));
      try {
        await owner.query(`CREATE SCHEMA ${schema}`); cli('migrate'); cli('migrate');
        server = spawn(binary, ['--config', config, 'serve'], { env, stdio: 'ignore' });
        for (let i = 0; i < 100; i++) { try { if ((await fetch(origin + '/health/ready')).ok) break; } catch {} await new Promise(resolve => setTimeout(resolve, 100)); }
        const page = await context.newPage(); const errors = []; page.on('pageerror', e => errors.push(e.message));
        await context.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);
        await page.goto(origin + '/setup'); assert.equal(await page.locator('input[name="token"]').count(),0); const flag = page.getByLabel('Installa dati demo', { exact: true }); assert.equal(await flag.isChecked(), false); assert.equal(await flag.isEnabled(), true);
        const password = randomBytes(20).toString('hex');
        const fill = async (storeName='Demo seeders') => { await page.getByLabel('Nome del negozio', { exact: true }).fill(storeName); await page.getByLabel('Nome visualizzato', { exact: true }).fill('Persona demo'); await page.getByLabel('Email amministratore').fill('demo-owner@example.test'); await page.getByLabel('Password', { exact: true }).fill(password); };
        if (withDemo) {
          await flag.focus(); await page.keyboard.press('Space'); assert.equal(await flag.isChecked(), true);
          await fill('   '); const rejected = page.waitForResponse(r => new URL(r.url()).pathname === '/setup' && r.request().method() === 'POST'); await page.getByRole('button', { name: 'Crea negozio' }).click(); assert.equal((await rejected).status(), 422); await page.getByRole('alert').waitFor(); assert.equal(await flag.isChecked(), true);
          assert.equal((await query('SELECT installed FROM identity_state')).rows[0].installed, false);
        }
        await fill();
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
        if (!process.env.TALOCART_TEST_NO_SCREENSHOTS) { const shot = { path: path.join(output, `setup-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        await page.getByRole('button', { name: 'Crea negozio' }).click(); await page.waitForURL('**/admin/login');
        assert.equal(Number((await query('SELECT count(*) FROM administrators')).rows[0].count), 1);
        if (!withDemo) {
          assert.equal(Number((await query('SELECT count(*) FROM catalog_products')).rows[0].count), 0);
          const before = (await query('SELECT row_to_json(p)::text AS data FROM pricing_settings p')).rows[0].data;
          assert.ok(cli('seed-demo', '--development', '--dry-run').includes('Preview')); assert.equal(fs.existsSync(media), false);
          assert.equal(Number((await query('SELECT count(*) FROM catalog_demo_packages')).rows[0].count), 0);
          const unconfirmed = spawnSync(binary, ['--config', config, 'seed-demo'], { env, encoding: 'utf8' }); assert.notEqual(unconfirmed.status, 0);
          assert.ok(cli('seed-demo', '--development').includes('Installed'));
          assert.equal((await query('SELECT row_to_json(p)::text AS data FROM pricing_settings p')).rows[0].data, before);
          await query("WITH u AS (UPDATE catalog_products SET name='Tazza modificata' WHERE model='MUG' RETURNING id,name) UPDATE catalog_products_translations t SET name=u.name FROM u WHERE t.entity_id=u.id AND t.locale=(SELECT primary_locale FROM storefront_settings WHERE id=1)");
          const files = fs.readdirSync(path.join(media, 'objects')).sort(); const hashes = files.map(f => createHash('sha256').update(fs.readFileSync(path.join(media, 'objects', f))).digest('hex'));
          assert.ok(cli('seed-demo', '--development').includes('AlreadyInstalled'));
          assert.equal((await query("SELECT name FROM catalog_products WHERE model='MUG'")).rows[0].name, 'Tazza modificata');
          assert.deepEqual(fs.readdirSync(path.join(media, 'objects')).sort(), files); assert.deepEqual(files.map(f => createHash('sha256').update(fs.readFileSync(path.join(media, 'objects', f))).digest('hex')), hashes);
        }
        assert.equal(Number((await query('SELECT count(*) FROM catalog_products')).rows[0].count), 30); assert.equal(Number((await query('SELECT count(*) FROM catalog_variants')).rows[0].count), 4);
        for (const table of ['catalog_products', 'catalog_categories', 'catalog_brands']) {
          assert.equal(Number((await query(`SELECT count(*) FROM ${table} WHERE slug ILIKE '%demo%'`)).rows[0].count), 0, `${table}: public slugs are clean`);
        }
        await page.getByLabel('Email amministratore').fill('demo-owner@example.test'); await page.getByLabel('Password', { exact: true }).fill(password); await page.getByRole('button', { name: 'Accedi', exact: true }).click(); await page.waitForURL('**/admin');
        if (process.env.TALOCART_DEMO_IMAGES_ONLY) {
          await require('./browser-gallery.cjs').images({page, query, origin, js, output, name});
          assert.deepEqual(errors, []);
          results.push(`${name}: direct images/ETag, small logos, local photo/history/form preservation, EN/IT, themes and footer build OK`); continue;
        }
        if (process.env.TALOCART_DEMO_VARIANT_COMMERCE_ONLY) {
          await require('./browser-product-quantities.cjs').variantSummary({page,query,origin,js});
          await require('./browser-product-quantities.cjs').stockPolicy({page,query,origin,js});
          await require('./browser-toasts.cjs')({page,query,origin,js,output,name});
          assert.deepEqual(errors, []);
          results.push(`${name}: stock policy, variant price/stock summaries, preserved parent values, EN/IT, themes and native forms OK`); continue;
        }
        if (process.env.TALOCART_DEMO_RELATED_ONLY) {
          await require('./browser-related-products.cjs')({page,query,origin,output,name,js});
          assert.deepEqual(errors, []);
          results.push(`${name}: related products EN/IT, themes, native forms and cards OK`); continue;
        }
        if (process.env.TALOCART_DEMO_LAYOUT_ONLY) {
          await require('./browser-layout.cjs')({page,origin,viewport,output,name,query,js});
          assert.deepEqual(errors, []);
          results.push(`${name}: responsive layout and table geometry OK`); continue;
        }
        if (process.env.TALOCART_DEMO_MEDIA_PICKER_ONLY) {
          await require('./browser-media-picker.cjs')({page,query,origin,name,js,output});
          assert.deepEqual(errors, []);
          results.push(`${name}: shared picker tree/upload/drafts, EN/IT, themes and compact Select OK`); continue;
        }
        if (process.env.TALOCART_DEMO_MEDIA_SIZING_ONLY) {
          await require('./browser-media-sizing.cjs')({page,query,origin,name,output});
          assert.deepEqual(errors, []);
          results.push(`${name}: native preview size, aspect ratios, bounded cards/detail/pickers, EN/IT and both themes OK`); continue;
        }
        if (process.env.TALOCART_DEMO_MEDIA_TRASH_ONLY) {
          await require('./browser-media-trash.cjs')({page,query,origin,name,output});
          assert.deepEqual(errors, []);
          results.push(`${name}: immediate trash purge, confirmation, EN/IT, both themes and native forms OK`); continue;
        }
        if (process.env.TALOCART_DEMO_MEDIA_TREE_ONLY) {
          await require('./browser-media-tree.cjs')({page,query,origin,name,js,output,mediaRoot:media});
          assert.deepEqual(errors, []);
          results.push(`${name}: folder tree, EN/IT, keyboard and native fallback OK`); continue;
        }
        if (process.env.TALOCART_DEMO_MEDIA_FILES_ONLY) {
          await require('./browser-media.cjs')({page,query,origin,name,js,output,mediaRoot:media});
          await require('./browser-media-files.cjs')({page,query,origin,name,output,mediaRoot:media});
          assert.deepEqual(errors, []);
          results.push(`${name}: automatic files, root/nested folders, documents, EN/IT OK`); continue;
        }
        if (process.env.TALOCART_DEMO_MEDIA_SPACING_ONLY) {
          await require('./browser-media-maintenance-spacing.cjs')({page,query,origin,name,output});
          assert.deepEqual(errors, []);
          results.push(`${name}: media maintenance spacing, EN/IT, keyboard and native controls OK`); continue;
        }
        if (process.env.TALOCART_DEMO_ADMIN_LISTS_ONLY) {
          await require('./browser-admin-lists.cjs')({page,origin,query,output,name});
          assert.deepEqual(errors, []);
          results.push(`${name}: white admin rows, pagination, EN/IT, palettes, densities, keyboard/no-JS OK`); continue;
        }
        if (process.env.TALOCART_DEMO_ADMIN_ICONS_ONLY) {
          await require('./browser-admin-icons.cjs')({page,origin,query,output,name,js});
          assert.deepEqual(errors, []);
          results.push(`${name}: admin menu/actions/edit icons, Italian/English, keyboard/native forms/responsive/no-JS OK`); continue;
        }
        if (process.env.TALOCART_DEMO_CONTENT_ONLY) {
          await require('./browser-content-localization.cjs')({ page, context, query, origin, output, name, js, viewport });
          assert.deepEqual(errors, []);
          results.push(`${name}: content translations, fallback, validation, keyboard, responsive, Italian/English OK`); continue;
        }
        if (process.env.TALOCART_DEMO_THEME_ONLY) {
          await require('./browser-admin-theme.cjs')({page,query,origin,output,name,js});
          results.push(`${name}: shared admin theme, 12 combinations, density/contrast/responsive/keyboard/HTMX OK`);continue;
        }
        if (process.env.TALOCART_DEMO_MEGA_ONLY) {
          await require('./browser-mega-menu.cjs')({page,query,origin,output,name,js,viewport});
          results.push(`${name}: three-level mega menu/responsive/keyboard/no-js OK`);continue;
        }
        if (process.env.TALOCART_DEMO_TREE_ONLY) {
          await require('./browser-category-tree.cjs')({page,query,origin,name,output,importsEnabled:false});
          results.push({name,category_tree:true,passed:true});continue;
        }
        if (process.env.TALOCART_DEMO_CATEGORY_ONLY) {
          await require('./browser-category-store.cjs')({ page, query, origin, js, output, name });
          await require('./browser-category-descendants.cjs')({ page, query, origin, js, output, name });
          assert.deepEqual(errors, []);
          results.push(`${name}: category descendants, Italian/English, filters/counts/pagination/search/brand/keyboard/responsive/no-JS OK`); continue;
        }
        if (process.env.TALOCART_DEMO_TAXONOMY_ONLY) {
          await require('./browser-taxonomy-lists.cjs')({ page, query, origin, name, output });
          results.push({name,taxonomy:true,passed:true});continue;
        }
        if (process.env.TALOCART_DEMO_LANGUAGE_POLICY_ONLY) {
          await require('./browser-store-language-policy.cjs')({page,context,query,origin,output,name,js,viewport});
          results.push(`${name}: active/primary language settings, validation, single English, redirects, secondary Italian, HTMX/gallery/history and no-JS OK`); continue;
        }
        await require('./browser-model-sku.cjs')({page,origin,query,output,name});
        if(process.env.TALOCART_DEMO_MODEL_SKU_ONLY){results.push(`${name}: model/SKU EN/IT, independent codes, forms, filters, public detail OK`);continue;}
        await require('./browser-toasts.cjs')({page,origin,name,js,output,query});
        if (process.env.TALOCART_DEMO_TOASTS_ONLY) { assert.deepEqual(errors, []); results.push(`${name}: toasts EN/IT OK`); continue; }
        await require('./browser-icons.cjs')({page,origin,name,output});
        await require('./browser-admin-icons.cjs')({page,origin,query,output,name,js});
        if (process.env.TALOCART_DEMO_ICONS_ONLY) {
          assert.deepEqual(errors, []);
          results.push(`${name}: Lucide examples, Italian/English, inherited colour, keyboard/accessibility/responsive/no-JS OK`); continue;
        }
        if (process.env.TALOCART_DEMO_UI_ONLY) {
          await require('./browser-ui-components.cjs')({page,origin,name,js,output});
          assert.deepEqual(errors, []);
          results.push(`${name}: shared UI and Lucide icon examples OK`); continue;
        }
        await require('./browser-store-localization.cjs')({page,origin,output,name,js,viewport});
        if (process.env.TALOCART_DEMO_STORE_LANGUAGE_ONLY) { results.push(`${name}: store localization, equivalent URLs, filters, native forms, variants/gallery HTMX, responsive and no-JS OK`); continue; }
        await require('./browser-store-language-policy.cjs')({page,context,query,origin,output,name,js,viewport});
        await require('./browser-admin-theme.cjs')({page,query,origin,output,name,js});
        await require('./browser-taxonomy-lists.cjs')({ page, query, origin, name, output });
        await require('./browser-category-tree.cjs')({page,query,origin,name,output});
        await require('./browser-ui-components.cjs')({ page, origin, name, js, output });
        await require('./browser-media.cjs')({ page, query, origin, name, js, output, mediaRoot: media });
        await require('./browser-layout.cjs')({ page, origin, viewport, output, name, query, js });
        await require('./browser-admin-lists.cjs')({page,origin,query,output,name});
        await page.goto(origin + '/admin/catalog/products'); assert.equal(await page.locator('.product-table tbody tr').count(), 20);
        await page.getByRole('navigation', { name: 'Pagine prodotti' }).getByRole('link', { name: 'Successivo', exact: true }).click();
        await page.waitForURL(url => url.pathname === '/admin/catalog/products' && url.searchParams.get('page') === '2'); assert.equal(await page.locator('.product-table tbody tr').count(), 10);
        await page.getByRole('navigation', { name: 'Pagine prodotti' }).getByRole('link', { name: 'Precedente', exact: true }).click();
        await page.waitForURL(url => url.pathname === '/admin/catalog/products' && url.searchParams.get('page') === '1'); assert.equal(await page.locator('.product-table tbody tr').count(), 20);
        assert.deepEqual(await page.locator('.admin-sidebar nav > details > summary').allTextContents(), ['Clienti', 'Catalogo', 'Design', 'Impostazioni', 'Utenti', 'Manutenzione']);
        const spacing = await page.locator('.admin-sidebar nav').evaluate(nav => {
          const groups = [...nav.querySelectorAll(':scope > details')];
          const states = groups.map(group => group.open);
          for (const group of groups) group.open = true;
          const measurements = {
            gap: parseFloat(getComputedStyle(nav).gap),
            items: [...nav.querySelectorAll(':scope > a, :scope > details > summary, .sidebar-submenu a')].map(e => ({ height: e.getBoundingClientRect().height, padding: parseFloat(getComputedStyle(e).paddingTop) })),
          };
          groups.forEach((group, index) => { group.open = states[index]; });
          return measurements;
        });
        assert.ok(spacing.gap <= 3, 'Sidebar groups use compact spacing');
        assert.ok(spacing.items.every(e => e.height >= 24 && e.height <= 36 && e.padding <= 6), `Sidebar links and submenus use compact padding (${name}: ${JSON.stringify(spacing)})`);
        { const shot = { path: path.join(output, `backend-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        const mug = (await query("SELECT id FROM catalog_products WHERE model='MUG'")).rows[0].id;
        const productPath = `/admin/catalog/products/${mug}`;
        const section = async label => {
          if (js) await page.getByRole('tab', { name: label, exact: true }).click();
          else await page.locator('.product-tab-editor .ui-tabs').getByRole('link', { name: label, exact: true }).click();
        };
        const active = async key => {
          if (js) {
            await page.locator('.product-tab-editor [data-ui-tabs-ready]').waitFor();
            await page.locator(`#panel-${key}`).waitFor({ state: 'visible' });
            assert.equal(await page.locator('[data-product-panel]:visible').count(), 1);
            assert.equal(await page.locator(`#panel-${key}`).isVisible(), true);
            assert.equal(await page.locator(`#tab-${key}`).getAttribute('aria-selected'), 'true');
          }
          assert.equal(await page.getByRole('button', {name:'Salva prodotto',exact:true}).isVisible(), true, `Global save is available in ${key}`);
          assert.ok(await page.getByRole('button', {name:'Salva prodotto',exact:true}).evaluate(button => button.form?.classList.contains('product-editor')), 'Global save submits the general product form');
          const belowPanel = await page.locator('[data-product-save]').evaluate((footer, key) => footer.getBoundingClientRect().top >= document.querySelector(`#panel-${key}`).getBoundingClientRect().bottom, key);
          assert.ok(belowPanel, `Global save follows the active ${key} panel`);
        };
        await page.goto(origin + productPath);
        assert.equal(await page.locator('.product-tab-editor .ui-tabs__tab').count(), 8); await active('general');
        const originalName = await page.getByLabel('Nome del prodotto', { exact: true }).inputValue();
        await page.getByLabel('Nome del prodotto', { exact: true }).fill('Modifica non salvata');
        await section('Prezzo e disponibilità'); await active('commerce');
        const originalPrice = await page.getByLabel('Prezzo unitario (€)', { exact: true }).inputValue();
        await page.getByLabel('Prezzo unitario (€)', { exact: true }).fill('17,50');
        await section('Organizzazione'); await active('organization');
        await section('Generali');
        assert.equal(await page.getByLabel('Nome del prodotto', { exact: true }).inputValue(), 'Modifica non salvata');
        assert.equal(await page.getByLabel('Prezzo unitario (€)', { exact: true }).inputValue(), '17,50');
        await page.getByLabel('Nome del prodotto', { exact: true }).fill(originalName);
        if (js) {
          await page.getByRole('tab', { name: 'Generali', exact: true }).focus(); await page.keyboard.press('ArrowRight');
          await active('commerce'); assert.equal(await page.getByRole('tab', { name: 'Prezzo e disponibilità', exact: true }).evaluate(e => e === document.activeElement), true);
          await page.getByLabel('Prezzo unitario (€)', { exact: true }).fill('');
          await section('Generali'); await page.getByRole('button', { name: 'Salva prodotto', exact: true }).click();
          await active('commerce');
          assert.equal(await page.getByLabel('Prezzo unitario (€)', { exact: true }).evaluate(e => e === document.activeElement), true);
        } else { assert.equal(await page.locator('[data-product-panel]:visible').count(), 8); await section('Prezzo e disponibilità'); }
        await page.getByLabel('Prezzo unitario (€)', { exact: true }).fill('1.234');
        await section('Generali');
        const invalidPrice = page.waitForResponse(r => new URL(r.url()).pathname === productPath && r.request().method() === 'POST');
        await page.getByRole('button', { name: 'Salva prodotto', exact: true }).click(); assert.equal((await invalidPrice).status(), 422);
        await active('commerce'); assert.equal(await page.getByLabel('Prezzo unitario (€)', { exact: true }).inputValue(), '1.234');
        await section('Prezzo e disponibilità'); await page.getByLabel('Prezzo unitario (€)', { exact: true }).fill(originalPrice);
        await page.getByRole('button', { name: 'Salva prodotto', exact: true }).click(); await page.waitForURL('**' + productPath + '?saved=1&tab=*');
        if (js) await active('commerce');
        await section('Generali'); await page.getByLabel('Nome del prodotto', {exact:true}).fill(originalName + ' da immagine');
        await section('Immagine'); await active('image');
        const imageBeforeSave = (await query('SELECT image_key FROM catalog_products WHERE id=$1',[mug])).rows[0].image_key;
        await page.getByRole('button', {name:'Salva prodotto',exact:true}).click();
        await page.waitForURL(url => url.searchParams.get('saved') === '1' && url.searchParams.get('tab') === (js ? 'image' : 'commerce'));
        await active(js ? 'image' : 'commerce');
        const savedFromImage = (await query('SELECT name,image_key FROM catalog_products WHERE id=$1',[mug])).rows[0];
        assert.equal(savedFromImage.name, originalName + ' da immagine'); assert.equal(savedFromImage.image_key, imageBeforeSave);
        await section('Immagine');
        assert.equal(await require('./media-picker-helper.cjs')(page, 'Seleziona immagine prodotto', 'crates/web/tests/fixtures/webp-animated.webp'), 422);
        await page.getByRole('alert').waitFor();
        await page.goto(origin + productPath + '?tab=image'); await active('image');
        await require('./browser-gallery.cjs')({ page, query, origin, id: mug, js, output, name });
        await section('Generali'); await page.getByLabel('Nome del prodotto', {exact:true}).fill(originalName);
        await section('Opzioni e varianti'); await active('variants');
        await page.getByRole('button', {name:'Salva prodotto',exact:true}).click();
        await page.waitForURL(url => url.searchParams.get('saved') === '1' && url.searchParams.get('tab') === (js ? 'variants' : 'image'));
        await active(js ? 'variants' : 'image');
        assert.equal((await query('SELECT name FROM catalog_products WHERE id=$1',[mug])).rows[0].name, originalName);
        await section('Opzioni e varianti');
        const colorId = (await query("SELECT id FROM catalog_options WHERE name='Colore'")).rows[0].id;
        await page.getByLabel('Opzione 1', { exact: true }).selectOption(String(colorId)); await page.getByLabel('Opzione 2', { exact: true }).selectOption(String(colorId));
        const invalidOptions = page.waitForResponse(r => new URL(r.url()).pathname === productPath + '/options' && r.request().method() === 'POST');
        await page.getByRole('button', { name: 'Salva opzioni prodotto', exact: true }).click(); assert.equal((await invalidOptions).status(), 422); await active('variants');
        await section('Eliminazione'); await active('delete');
        const invalidDelete = page.waitForResponse(r => new URL(r.url()).pathname === productPath + '/delete' && r.request().method() === 'POST');
        const deleteNavigation = page.waitForNavigation({ waitUntil: 'domcontentloaded' });
        await page.locator('#panel-delete form').evaluate(form => { form.noValidate = true; form.requestSubmit(); });
        assert.equal((await invalidDelete).status(), 422); await deleteNavigation; await active('delete');
        await page.goto(origin + productPath); await active('general'); assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
        { const shot = { path: path.join(output, `product-tabs-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        await section('Immagine'); { const shot = { path: path.join(output, `product-tabs-image-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        await require('./browser-header-search.cjs')({ page, origin, viewport, output, name });
        await require('./browser-mega-menu.cjs')({page,query,origin,output,name,js,viewport});
        // Public top menu: real native disclosures, keyboard, filtering and no-JS fallback.
        await query("INSERT INTO catalog_categories(name,slug) VALUES ('Menu · Extra','menu-extra')");
        await query("INSERT INTO catalog_categories(name,slug,parent_id) SELECT 'Menu · Prova','menu-prova',id FROM catalog_categories WHERE slug='menu-extra'");
        await query("INSERT INTO catalog_categories(name,slug,parent_id) SELECT 'Menu · Fogli','menu-fogli',id FROM catalog_categories WHERE slug='cancelleria'");
        const mobile = viewport.width <= 800;
        const openMenu = async () => {
          const navigation = page.locator('.category-navigation');
          if (!(await navigation.evaluate(el => el.open))) {
            await navigation.locator(':scope > summary').focus(); await page.keyboard.press('Enter');
          }
        };
        const dropdown = label => page.locator('.category-dropdown').filter({ has: page.locator('summary', { hasText: label }) });
        await page.goto(origin + '/');
        if (js && mobile) assert.equal(await page.locator('.category-navigation').evaluate(el => el.open), false);
        await openMenu();
        const menu = page.getByRole('navigation', { name: 'Categorie del catalogo', exact: true });
        assert.ok((await menu.locator('a').evaluateAll(links => links.map(a => a.getAttribute('href')))).every(href => !href.includes('demo-')), 'Menu links use clean category slugs');
        assert.equal(await menu.getByText('Archivio inattivo', { exact: true }).count(), 0);
        const accessories = dropdown('Accessori');
        await accessories.locator('summary').focus(); await page.keyboard.press('Enter');
        assert.equal(await accessories.evaluate(el => el.open), true);
        assert.equal(await accessories.getByRole('link', { name: 'Cancelleria', exact: true }).isVisible(), true);
        assert.equal(await accessories.getByRole('link', { name: 'Menu · Fogli', exact: true }).isVisible(), true);
        const viewportBefore=await page.evaluate(() => ({width:innerWidth,height:innerHeight,desktop:matchMedia('(min-width:801px)').matches,open:document.querySelector('.category-dropdown[open]')?.querySelector('summary').textContent}));
        // Full-page capture can transiently emulate a 1x1 viewport in Chrome,
        // crossing the mobile breakpoint and legitimately closing the menu.
        // Capture this interaction at the actual viewport; explicit resize tests stay below.
        { const shot = { path: path.join(output, `category-menu-${name}.png`) }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        const viewportAfter=await page.evaluate(() => ({width:innerWidth,height:innerHeight,desktop:matchMedia('(min-width:801px)').matches,open:document.querySelector('.category-dropdown[open]')?.querySelector('summary').textContent}));
        assert.equal(await accessories.evaluate(el => el.open), true, 'Viewport capture preserves the open category panel '+JSON.stringify({name,viewportBefore,viewportAfter}));
        if (js) {
          await accessories.getByRole('link', { name: 'Cancelleria', exact: true }).focus(); await page.keyboard.press('Escape');
          assert.equal(await accessories.evaluate(el => el.open), false);
          assert.ok(await accessories.locator('summary').evaluate(el => el === document.activeElement));
          await page.keyboard.press('Enter');
          const extra = dropdown('Menu · Extra'); await extra.locator('summary').click();
          assert.equal(await accessories.evaluate(el => el.open), false); assert.equal(await extra.evaluate(el => el.open), true);
          await page.getByLabel('Cerca un prodotto', { exact: true }).click(); assert.equal(await extra.evaluate(el => el.open), false);
          await accessories.locator('summary').click();
        }
        await accessories.getByRole('link', { name: 'Cancelleria', exact: true }).click();
        await page.waitForURL('**/catalog/cancelleria');
        assert.equal(await page.locator('.store-product').count(), 9);
        await openMenu(); await dropdown('Accessori').locator('summary').click();
        assert.equal(await menu.locator('a[aria-current]').innerText(), 'Cancelleria');
        await page.getByLabel('Cerca un prodotto', { exact: true }).fill('Quaderno'); await page.getByRole('button', { name: 'Cerca', exact: true }).click();
        await page.waitForURL(u => u.pathname === '/search/Quaderno');
        assert.equal(new URL(page.url()).searchParams.get('category'), 'cancelleria'); assert.equal(await page.locator('.store-product').count(), 8);
        await openMenu(); await dropdown('Accessori').locator('summary').click();
        await dropdown('Accessori').locator('.category-root-link').click();
        await page.waitForURL('**/catalog/accessori'); assert.equal(await page.locator('.store-product').count(), 21);
        assert.equal(await page.locator('.catalog-result-count').textContent(), '21 prodotti');
        assert.equal(new URL(page.url()).searchParams.has('q'), false);
        await query("UPDATE catalog_categories SET active=false WHERE slug='accessori'");
        await page.goto(origin + '/'); assert.equal(await menu.getByText('Accessori', { exact: true }).count(), 0); assert.equal(await menu.getByText('Menu · Fogli', { exact: true }).count(), 0);
        await query("UPDATE catalog_categories SET active=true WHERE slug='accessori'");
        await page.goto(origin + '/catalog'); assert.equal(await page.locator('.store-product').count(), 21);
        await page.getByRole('navigation', { name: 'Pagine catalogo' }).getByRole('link', { name: 'Successivo', exact: true }).click();
        await page.waitForURL(url => url.pathname === '/catalog' && url.searchParams.get('page') === '2'); assert.equal(await page.locator('.store-product').count(), 6);
        await page.getByRole('navigation', { name: 'Pagine catalogo' }).getByRole('link', { name: 'Precedente', exact: true }).click();
        await page.waitForURL(url => url.pathname === '/catalog' && url.searchParams.get('page') === '1'); assert.equal(await page.locator('.store-product').count(), 21);
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)); { const shot = { path: path.join(output, `catalog-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        assert.equal((await context.request.get(origin + '/products/demo-tazza-atelier')).status(), 404, 'Old demo URLs are not emitted or silently served');
        await page.goto(origin + '/products/tazza-atelier'); assert.equal(await page.locator('[data-product-price]').innerText(), '14,90 € IVA inclusa'); assert.ok(await page.locator('.store-detail-photo img').evaluate(img => img.complete && img.naturalWidth > 0));
        await page.goto(origin + '/products/tshirt-atelier');
        for (const [label, option] of [['Colore', 'Blu'], ['Taglia', 'M']]) {
          const select = page.getByLabel(label, { exact: true }); await select.focus();
          if (js) { await page.evaluate(() => { window.demoSettled = false; document.body.addEventListener('htmx:afterSettle', () => { window.demoSettled = true; }, { once: true }); }); }
          await select.selectOption({ label: option });
          if (js) { await page.waitForFunction(() => window.demoSettled === true); assert.ok(await page.getByLabel(label, { exact: true }).evaluate(el => el === document.activeElement)); }
        }
        if (!js) { await page.getByRole('button', { name: 'Mostra variante', exact: true }).click(); await page.waitForURL('**/products/tshirt-atelier?**'); }
        assert.equal(await page.locator('[data-product-sku]').innerText(), 'SHIRT-BLUE-M'); assert.equal(await page.locator('[data-product-price]').innerText(), '26,90 € IVA inclusa');
        assert.ok(new URL(page.url()).searchParams.get('value_1')); assert.ok(new URL(page.url()).searchParams.get('value_2'));
        assert.ok(await page.locator('.store-detail-photo img').evaluate(img => img.complete && img.naturalWidth > 0)); assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)); { const shot = { path: path.join(output, `variant-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        await require('./browser-gallery.cjs').variant({ page, query, origin, js });
        for (const slug of ['prodotto-bozza', 'categoria-disattivata', 'varianti-da-completare']) assert.equal((await context.request.get(origin + '/products/' + slug)).status(), 404);
        await openMenu();
        assert.equal(await page.locator('.category-menu-item.is-selected').innerText(), 'Abbigliamento');
        // Wrapping and long labels stay inside the viewport; all categories remain reachable.
        await query("INSERT INTO catalog_categories(name,slug) SELECT 'Categoria di prova con nome molto lungo '||n,'menu-lunga-'||n FROM generate_series(1,18) n");
        await page.goto(origin + '/'); await openMenu(); assert.equal(await menu.locator('.category-menu-item').count(), 22);
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
        { const shot = { path: path.join(output, `category-menu-many-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        if (js && mobile) {
          await page.locator('.category-navigation-toggle').focus(); await page.keyboard.press('Escape');
          assert.equal(await page.locator('.category-navigation').evaluate(el => el.open), false);
          await page.setViewportSize({ width: 1440, height: 1050 });
          await page.waitForFunction(() => document.querySelector('.category-navigation').open);
          await page.setViewportSize(viewport);
          await page.waitForFunction(() => !document.querySelector('.category-navigation').open);
        }
        await require('./browser-product-quantities.cjs')({ page, query, origin, id: mug, js, output, name });
        await require('./browser-catalog-layout.cjs')({ page, origin, output, name });
        await require('./browser-catalog-pagination.cjs')({ page, origin, js, output, name });
        await require('./browser-category-store.cjs')({ page, query, origin, js, output, name });
        await require('./browser-category-descendants.cjs')({ page, query, origin, js, output, name });
        await require('./browser-brand-store.cjs')({ page, query, origin, js, output, name });
        await require('./browser-search-store.cjs')({ page, query, origin, js, output, name });
        await require('./browser-product-card.cjs')({ page, query, origin, output, name });
        await require('./browser-card-images.cjs')({ browser, origin, js });
        await require('./browser-demo-storefront.cjs')({ page, query, origin, id:mug, output, name, js });
        await require('./browser-unlimited-options.cjs')({ page, query, origin, js, output, name });
        await require('./browser-product-bulk.cjs')({ page, query, origin, js, output, name });
        await require('./browser-optional-category.cjs')({ page, query, origin, js, output, name });
        await require('./browser-bulk-category.cjs')({ page, query, origin, js, output, name });
        await require('./browser-pagination-extension.cjs')({page,query,origin,name,output,mediaRoot:media});
        await require('./browser-content-localization.cjs')({ page, context, query, origin, output, name, js, viewport });
        assert.deepEqual(errors, []);
        results.push(`${name}: content translations, shared admin theme/12 combinations/density/contrast/responsive/HTMX, shared UI examples/native GET/errors/keyboard/double-border/focus, setup opt-in/default, keyboard, validation, product tabs/keyboard/retained fields/error context, compact sidebar, category menu/hierarchy/keyboard/filter/current/live updates/long labels, gallery upload/order/primary/removal/keyboard/focus/variant selection, demo catalog/images/variants, shared product card 3:4/home/catalog/mobile/keyboard/no-js, catalog sidebar/three-column grid/responsive/filter/keyboard/empty/pagination, theme/SEO/contrast/focus/filters/sitemap and protected cases, optional-category publication/single/bulk/public/filter, bulk category tree/replacement/published+draft/rollback, bulk actions count-only/preview/cancel/confirmation/delete/publish/draft/rollback/retained filters, unlimited options/values/variants beyond 100/100/500${withDemo ? '' : ', CLI preview/idempotence/preserved edits'} OK`);
      } finally {
        await context.close(); if (server && server.exitCode === null) { const done = once(server, 'exit'); server.kill('SIGTERM'); const [code] = await done; assert.equal(code, 0); }
        await owner.query('SET search_path TO public'); await owner.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`); fs.rmSync(folder, { recursive: true, force: true });
      }
    }
    fs.writeFileSync(path.join(output, 'results.json'), JSON.stringify(results, null, 2)); console.log(results.join('\n'));
  } finally { await browser.close(); await owner.end(); }
})().catch(e => { console.error(e.message); console.error((e.stack || '').split('\n').filter(l => /scripts\/browser-[a-z-]+\.cjs:/.test(l)).join('\n')); process.exitCode = 1; });
