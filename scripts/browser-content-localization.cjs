const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, context, query, origin, name, output, viewport, js }) => {
  const fillDescription = async (field, value) => {
    if (js && await field.locator('xpath=..').getAttribute('data-rich-ready') === 'true') {
      await field.locator('xpath=..').locator('.tiptap').fill(value);
    } else await field.fill(value);
  };
  const product = (await query("SELECT id,category_id,brand_id,stock,minimum_quantity FROM catalog_products WHERE slug='tshirt-atelier'")).rows[0];
  const option = (await query("SELECT id FROM catalog_options WHERE name='Colore'")).rows[0];
  const value = (await query("SELECT id FROM catalog_option_values WHERE option_id=$1 AND name='Blu'", [option.id])).rows[0];
  for (const resource of ['categories', 'brands', 'products', 'options']) {
    await page.goto(`${origin}/admin/catalog/${resource}/new`);
    assert.equal(await page.locator('[name="content_en_name"]').count(), 1, `Both languages already in new ${resource}`);
    assert.equal(await page.locator('a[href^="/admin/content/"]').count(), 0);
  }
  const cases = [
    [`/admin/catalog/categories/${product.category_id}`, 'catalog-name', 'name', 'description', 'seo_title', 'Abbigliamento', 'Clothing', 'Salva categoria'],
    [`/admin/catalog/brands/${product.brand_id}`, 'catalog-name', 'name', 'description', 'seo_title', 'Talocart Atelier', 'English atelier', 'Salva marchio'],
    [`/admin/catalog/products/${product.id}`, 'product-name', 'name', 'description', 'seo_title', 'Maglietta italiana', 'English shirt', 'Salva prodotto'],
    [`/admin/catalog/options/${option.id}`, 'option-name', 'name', '', '', 'Colore', 'Colour', 'Salva opzione'],
    [`/admin/catalog/options/${option.id}`, `value-${value.id}`, 'name', '', '', 'Blu', 'Blue', 'Salva valore'],
    ['/admin/settings/storefront', 'theme-heading', 'heading', 'introduction', 'seo_title', 'Benvenuti', 'Welcome home', 'Salva impostazioni'],
  ];
  for (const [route, anchor, primaryField, description, seo, italian, english, button] of cases) {
    await page.goto(origin + route);
    const form = page.locator(`form:has(#${anchor})`);
    assert.equal(await page.locator('html').getAttribute('lang'), 'it');
    await form.locator(`[name="${primaryField}"]`).fill(italian);
    await form.locator('[name="content_en_name"]').fill(english);
    if (description && await form.locator(`[name="${description}"]`).count()) {
      await fillDescription(form.locator(`[name="${description}"]`), 'Descrizione principale');
      await fillDescription(form.locator('[name="content_en_description"]'), '');
    }
    if (seo && await form.locator(`[name="${seo}"]`).count()) {
      await form.locator(`[name="${seo}"]`).fill('Titolo SEO italiano');
      await form.locator('[name="content_en_seo_title"]').fill('English SEO title');
    }
    const save = anchor === 'product-name' ? page.getByRole('button', { name: button, exact: true }) : form.getByRole('button', { name: button, exact: true });
    await save.focus(); assert.ok(await save.evaluate(el => el === document.activeElement));
    await page.keyboard.press('Enter');
    await page.waitForURL(url => url.pathname === route && url.searchParams.get('saved') === '1');
    await (route.startsWith('/admin/settings') ? page.locator('#settings-saved') : page.getByRole('status')).waitFor();
    assert.equal(await page.locator(`form:has(#${anchor}) [name="content_en_name"]`).inputValue(), english);
  }
  const route = `/admin/catalog/products/${product.id}`;
  await page.goto(origin + route);
  await page.locator('[name="content_en_name"]').fill('Bad\tname');
  await fillDescription(page.locator('[name="content_en_description"]'), 'Preserved after error');
  await page.locator('#tab-commerce').click();
  // Configurable products expose stock through variants; purchase rules remain
  // editable on the parent and must survive unrelated translation errors.
  await page.locator('[name=minimum_quantity]').fill('55');
  const invalid = page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === route);
  await page.getByRole('button', { name: 'Salva prodotto', exact: true }).click();
  assert.equal((await invalid).status(), 422);
  assert.equal(await page.locator('[data-product-tabs]').getAttribute('data-active-tab'), 'general');
  assert.equal(await page.locator('[name=minimum_quantity]').inputValue(), '55');
  assert.equal(Number((await query('SELECT stock FROM catalog_products WHERE id=$1', [product.id])).rows[0].stock), Number(product.stock));
  assert.equal(Number((await query('SELECT minimum_quantity FROM catalog_products WHERE id=$1', [product.id])).rows[0].minimum_quantity), Number(product.minimum_quantity));
  assert.equal(js ? await page.locator('#product-content-en-description-visual').innerText() : await page.locator('[name="content_en_description"]').inputValue(), 'Preserved after error');
  assert.equal(await page.locator('[name="content_en_name"]').getAttribute('aria-invalid'), 'true');
  assert.ok(await page.getByRole('alert').isVisible());
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.screenshot({ path: path.join(output, `content-inline-it-${name}.png`), fullPage: true });
  await page.goto(origin + '/admin/account');
  await page.locator('select[name="locale"]').selectOption('en');
  await page.getByRole('button', { name: 'Salva lingua', exact: true }).click();
  await page.waitForURL('**/admin/account?saved=1');
  await page.goto(origin + route);
  assert.equal(await page.locator('#product-name').inputValue(), 'Maglietta italiana');
  await page.locator('[name="content_en_name"]').fill('Invalid\tname');
  const englishInvalid = page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === route);
  await page.getByRole('button', { name: 'Save product', exact: true }).click();
  assert.equal((await englishInvalid).status(), 422);
  assert.equal(await page.getByRole('alert').innerText(), 'Check the highlighted fields and text limits.');
  await page.locator('[name="content_en_name"]').fill('English shirt');
  await page.getByRole('button', { name: 'Save product', exact: true }).click();
  await page.waitForURL(url => url.pathname === route && url.searchParams.get('saved') === '1');
  for (const width of [320, 390, 800, 1440]) {
    await page.setViewportSize({ width, height: 1050 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${name}: inline languages at ${width}px`);
  }
  await page.setViewportSize(viewport);
  await page.screenshot({ path: path.join(output, `content-inline-en-${name}.png`), fullPage: true });
  await page.goto(origin + '/en/products/tshirt-atelier');
  assert.ok(await page.getByRole('heading', { name: 'English shirt', exact: true }).isVisible());
  assert.ok((await page.locator('main').innerText()).includes('Descrizione principale'));
  assert.equal(await page.title(), 'English SEO title');
  for (const [label, choice] of [['Colour', 'Blue'], ['Taglia', 'M']]) {
    if (js) await page.evaluate(() => { window.contentSettled = false; document.body.addEventListener('htmx:afterSettle', () => { window.contentSettled = true; }, { once: true }); });
    await page.getByLabel(label, { exact: true }).selectOption({ label: choice });
    if (js) await page.waitForFunction(() => window.contentSettled);
  }
  if (!js) {
    await page.getByRole('button', { name: 'Show variant', exact: true }).click();
    await page.waitForURL(url => url.searchParams.has('value_2'));
  }
  assert.equal(await page.locator('[data-product-sku]').innerText(), 'SHIRT-BLUE-M');
  assert.equal(await page.locator('[data-product-price]').innerText(), '26.90 € VAT included');
  await page.goto(origin + '/en/search/English%20shirt'); assert.equal(await page.locator('.store-product').count(), 1);
  await page.goto(origin + '/en'); assert.ok(await page.getByRole('heading', { name: 'Welcome home', exact: true }).isVisible());
  // A real stale browser form must fail when active languages change, preserving input.
  const settingsPage = await context.newPage();
  const languages = async (primary, active) => {
    await settingsPage.goto(origin + '/admin/settings/storefront');
    const form = settingsPage.locator('#settings-editor');
    await form.locator('[name="primary_locale"]').selectOption(primary);
    for (const code of ['en', 'it']) await form.locator(`[name="locale"][value="${code}"]`).setChecked(active.includes(code));
    await settingsPage.locator('#settings-save').click();
    await settingsPage.waitForURL('**/admin/settings/storefront?saved=1');
  };
  for (const adminLocale of ['en', 'it']) {
    await page.goto(origin + '/admin/account');
    const languageForm = page.locator('form:has(select[name="locale"])');
    await languageForm.locator('select').selectOption(adminLocale);
    await languageForm.locator('button[type="submit"]').click();
    await page.waitForURL('**/admin/account?saved=1');
    await page.goto(origin + route);
    await page.locator('[name="content_en_name"]').fill('Unsaved translation');
    await languages('it', ['it']);
    const rejected = page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === route);
    await page.getByRole('button', {name: adminLocale === 'en' ? 'Save product' : 'Salva prodotto', exact:true}).click();
    assert.equal((await rejected).status(),409);
    assert.ok((await page.getByRole('alert').innerText()).includes(adminLocale === 'en' ? 'enabled languages' : 'lingue abilitate'));
    assert.equal(await page.locator('[name="content_en_name"]').inputValue(),'Unsaved translation');
    await page.goto(origin + route);
    assert.equal(await page.locator('[name="content_en_name"]').count(),0);
    assert.equal(await page.locator('[name="content_active_locales"]').inputValue(),'it');
    await page.getByRole('button', {name: adminLocale === 'en' ? 'Save product' : 'Salva prodotto', exact:true}).focus();
    await page.keyboard.press('Enter');
    await page.waitForURL(url => url.pathname===route && url.searchParams.get('saved')==='1');
    assert.equal((await query("SELECT name FROM catalog_products_translations WHERE entity_id=$1 AND locale='en'",[product.id])).rows[0].name,'English shirt');
    await languages('en',['en']);
    await page.goto(origin + route);
    assert.equal(await page.locator('[name="content_it_name"]').count(),0);
    assert.equal(await page.locator('#product-name').inputValue(),'English shirt');
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({path:path.join(output,`content-single-${adminLocale}-${name}.png`),fullPage:true});
    await languages('it',['it','en']);
    await page.goto(origin + route);
    assert.equal(await page.locator('[name="content_en_name"]').inputValue(),'English shirt');
  }
  await settingsPage.close();
  await page.goto(origin + '/admin/account'); await page.locator('select[name="locale"]').selectOption('it');
  await page.getByRole('button', { name: 'Salva lingua', exact: true }).click(); await page.waitForURL('**/admin/account?saved=1');
};
