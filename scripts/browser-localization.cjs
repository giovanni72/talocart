// Reuses the phase-1 isolated database, accounts, server and three browser profiles.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, context, origin, name, js, output, restartPage }) => {
  await page.goto(origin + '/admin/account');
  const select = page.getByRole('combobox', { name: 'Lingua dell’amministrazione', exact: true });
  assert.equal(await select.inputValue(), 'it');
  await select.focus();
  await select.selectOption('en');
  await page.getByRole('button', { name: 'Salva lingua', exact: true }).focus();
  await page.keyboard.press('Enter');
  await page.waitForURL('**/admin/account?saved=1');
  assert.equal(await page.locator('html').getAttribute('lang'), 'en');
  assert.equal(await page.getByRole('combobox', { name: 'Administration language', exact: true }).inputValue(), 'en');
  assert.equal(await page.getByLabel('Display name', { exact: true }).inputValue(), `Persona ${name}`);
  for (const width of [320, 390, 1440, 2560]) {
    await page.setViewportSize({ width, height: 900 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `English account fits ${width}px`);
  }
  await page.setViewportSize({ width: name === 'mobile' ? 390 : 1440, height: name === 'mobile' ? 844 : 1050 });
  await require('./ui-contract.cjs')(page);
  await page.waitForLoadState('networkidle');
  if (js) await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await page.screenshot({ path: path.join(output, `language-account-${name}.png`), fullPage: true });
  if (name === 'desktop') {
    await restartPage(page);
    assert.equal(await page.locator('html').getAttribute('lang'), 'en');
  }
  for (const route of ['/admin', '/admin/operations', '/admin/settings', '/admin/settings/storefront', '/admin/settings/appearance', '/admin/settings/taxes', '/admin/users', '/admin/roles', '/admin/catalog/categories', '/admin/catalog/brands', '/admin/catalog/products', '/admin/catalog/options', '/admin/modules', '/admin/status', '/admin/media', '/admin/components']) {
    const response = await page.goto(origin + route);
    assert.equal(response.status(), 200, route);
    if (route === '/admin') assert.equal(await page.locator('h1').innerText(), 'Overview');
    assert.equal(response.headers()['content-language'], 'en', route);
    assert.equal(await page.locator('html').getAttribute('lang'), 'en', route);
    if (route === '/admin/catalog/products') assert.equal(await page.getByRole('link', { name: 'Sort by Type in ascending order', exact: true }).count(), 1);
    assert.ok(await page.getByRole('navigation', { name: 'Administration', exact: true }).isVisible());
    const visible = await page.locator('body').innerText();
    assert.ok(!/\b(?:admin|ops|dash)-[a-z]+/.test(visible), `No untranslated IDs: ${route}`);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), route);
  }
  assert.equal((await context.request.get(origin + '/admin/catalog/imports')).status(), 404, 'Disabled imports stay inaccessible');
  await page.goto(origin + '/admin/status');
  await page.getByRole('button', { name: 'Check now', exact: true }).click();
  if (js) {
    await page.locator('.status-content[data-state="ok"]').waitFor();
    assert.ok(await page.locator('.status-content').innerText().then(text => text.includes('The foundations are ready')));
  } else {
    await page.waitForURL(url => url.pathname === '/admin/status');
    assert.equal(await page.locator('html').getAttribute('lang'), 'en');
  }
  await page.goto(origin + '/admin/account');
  const validation = await context.newPage();
  const validationErrors = [];
  validation.on('pageerror', error => validationErrors.push(error.message));
  validation.on('console', message => { if (message.type() === 'error') validationErrors.push(message.text()); });
  await validation.goto(origin + '/admin/account');
  const rejected = validation.waitForResponse(response => response.url().endsWith('/admin/account/password') && response.request().method() === 'POST');
  await validation.getByLabel('Current password', { exact: true }).fill('unused');
  await validation.getByLabel('New password', { exact: true }).fill('valid new password');
  await validation.getByLabel('Confirm new password', { exact: true }).fill('different valid password');
  await validation.getByRole('button', { name: 'Update password', exact: true }).click();
  await validation.getByRole('alert').waitFor();
  assert.equal(await validation.getByRole('alert').innerText(), 'The new passwords do not match');
  assert.equal(await validation.locator('html').getAttribute('lang'), 'en');
  assert.equal((await rejected).status(), 422);
  assert.deepEqual(validationErrors, ['Failed to load resource: the server responded with a status of 422 (Unprocessable Entity)']);
  await validation.close();
  const publicPage = await context.newPage();
  await publicPage.goto(origin + '/');
  assert.equal(await publicPage.locator('html').getAttribute('lang'), 'it');
  await publicPage.close();
  await page.getByRole('combobox', { name: 'Administration language', exact: true }).selectOption('it');
  await page.getByRole('button', { name: 'Save language', exact: true }).click();
  await page.waitForURL('**/admin/account?saved=1');
  assert.equal(await page.locator('html').getAttribute('lang'), 'it');
  assert.equal(await page.getByRole('combobox', { name: 'Lingua dell’amministrazione', exact: true }).inputValue(), 'it');
};
