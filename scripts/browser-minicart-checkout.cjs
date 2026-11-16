// Reused by the cart journey: real checkout forms, with only response timing controlled.
const assert = require('node:assert/strict');
module.exports = async (page, navigate, locale) => {
  for (const timing of ['after', 'during', 'reopen', 'retry']) {
    await navigate('/products/cart-simple', locale);
    await page.getByRole('button', {name: locale === 'it' ? 'Aggiungi al carrello' : 'Add to cart', exact: true}).click();
    await navigate('/checkout', locale);
    await page.locator('#email').fill('minicart@example.test');
    await page.locator('#first_name').fill('Synthetic');
    await page.locator('#last_name').fill('Customer');
    await page.locator('#invoice_requested').check();
    const snapshot = () => page.locator('#checkout-details').evaluate(form => [...new FormData(form)].filter(([key]) => key !== 'revision'));
    let before = await snapshot();
    const revision = await page.locator('#checkout-details [name=revision]').inputValue();
    await page.evaluate(() => { window.minicartCheckoutDocument = true; });
    let release, started;
    const requested = new Promise(resolve => { started = resolve; });
    const gate = new Promise(resolve => { release = resolve; });
    const intercept = async route => {
      const response = await route.fetch();
      started();
      await gate;
      await route.fulfill({response});
    };
    if (timing === 'retry') await page.route('**/checkout', route => route.request().method() === 'POST' ? route.abort() : route.continue());
    if (timing !== 'after') await page.route('**/cart/items/*', intercept);
    await page.locator('[data-cart-link]').click();
    await page.locator('[data-minicart-remove]').click();
    if (timing === 'after') {
      await page.locator('#minicart-content [data-minicart-count="0"]').waitFor();
      await page.keyboard.press('Escape');
    } else {
      await requested;
      await page.keyboard.press('Escape');
      assert.equal(await page.locator('#checkout-details button[type=submit]').first().isDisabled(), true, 'Cannot submit an outdated revision while removal is pending');
      await page.locator('#last_name').fill('Edited during removal');
      before = await snapshot();
      if (timing === 'reopen') await page.locator('[data-cart-link]').click();
      release();
      await page.locator('#minicart-content [data-minicart-count="0"]').waitFor({state: 'attached'});
      if (timing === 'reopen') await page.keyboard.press('Escape');
      await page.unroute('**/cart/items/*', intercept);
    }
    if (timing === 'retry') {
      await page.locator('#checkout-cart-update button').waitFor();
      assert.deepEqual(await snapshot(), before, 'A failed refresh preserves the draft');
      await page.locator('main').screenshot({path: `.local/verification/cart/checkout-minicart-retry-${locale}-${page.viewportSize().width}.png`});
      assert.equal(await page.locator('#checkout-details button[type=submit]').first().isDisabled(), true);
      await page.unroute('**/checkout');
      await page.locator('#checkout-cart-update button').click();
    }
    await page.waitForFunction(old => document.querySelector('#checkout-details [name=revision]')?.value !== old, revision);
    assert.equal(await page.evaluate(() => window.minicartCheckoutDocument), true, 'Removal must not reload checkout');
    assert.deepEqual(await snapshot(), before, 'All unsaved fields and checkbox choices survive removal');
    assert.equal(await page.locator('#checkout-summary .checkout-lines li').count(), 0, 'Summary reflects the empty cart');
    assert.equal(await page.locator('#checkout-details button[type=submit]').first().isEnabled(), true);
    assert.equal(await page.locator('#minicart').evaluate(dialog => dialog.open), false);
  }
  // The cart page also refreshes when its removal completes after dismissal.
  await navigate('/products/cart-simple', locale);
  await page.getByRole('button', {name: locale === 'it' ? 'Aggiungi al carrello' : 'Add to cart', exact: true}).click();
  await navigate('/cart', locale);
  let release, started;
  const requested = new Promise(resolve => { started = resolve; });
  const gate = new Promise(resolve => { release = resolve; });
  const intercept = async route => { const response = await route.fetch(); started(); await gate; await route.fulfill({response}); };
  await page.route('**/cart/items/*', intercept);
  await page.locator('[data-cart-link]').click();
  await page.locator('[data-minicart-remove]').click();
  await requested;
  await page.keyboard.press('Escape');
  const refreshed = page.waitForEvent('load');
  release();
  await refreshed;
  assert.equal(await page.locator('[data-cart-item]').count(), 0);
  await page.unroute('**/cart/items/*', intercept);
};
