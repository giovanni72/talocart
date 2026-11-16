// Shared assertions for native charge-tax fields in the existing browser suites.
const assert = require('node:assert/strict');
exports.fixture = async db => {
  const rate = (await db.query("INSERT INTO tax_rates(name,basis_points) VALUES('Accessory test VAT',2200) RETURNING id")).rows[0].id;
  return String((await db.query("INSERT INTO tax_classes(name,rate_id) VALUES('Accessory test class',$1) RETURNING id",[rate])).rows[0].id);
};
exports.missingClass = async ({page,save,prefix,lang,classId}) => {
  const select=page.locator(`#${prefix}-tax-class`);
  await select.selectOption('');await save(422);
  assert.equal(await select.getAttribute('aria-invalid'),'true');
  assert.equal(await select.getAttribute('aria-describedby'),`${prefix}-tax-help ${prefix}-tax-error`);
  assert.ok((await page.locator(`#${prefix}-tax-error`).innerText()).includes(lang==='it'?'classe fiscale':'tax class'));
  await select.selectOption(classId);await select.focus();
  assert.equal(await select.evaluate(e=>e===document.activeElement),true);
  assert.ok(await select.evaluate(e=>getComputedStyle(e).outlineStyle!=='none'));
};
exports.mode = async ({page,db,save,prefix,lang}) => {
  assert.equal(await page.locator('input[name=price_mode]').inputValue(),'inclusive');
  assert.ok((await page.locator(`#${prefix}-fee-mode`).innerText()).includes(lang==='it'?'comprendono l’IVA':'include VAT'));
  if(Number((await db.query('SELECT count(*) FROM catalog_products')).rows[0].count)>0) {
    // The demo catalog intentionally locks price mode; verify that guard here.
    await assert.rejects(db.query("UPDATE pricing_settings SET price_mode='exclusive'"),e=>e.constraint==='pricing_settings_product_prices');
    return;
  }
  const url=page.url();const value=await page.locator(`#${prefix}-fee_value`).inputValue();
  await db.query("UPDATE pricing_settings SET price_mode='exclusive'");
  await save(409);
  assert.equal(await page.locator(`#${prefix}-fee_value`).inputValue(),value);
  await page.goto(url);
  assert.equal(await page.locator('input[name=price_mode]').inputValue(),'exclusive');
  assert.ok((await page.locator(`#${prefix}-fee-mode`).innerText()).includes(lang==='it'?'al netto dell’IVA':'exclude VAT'));
  await save();
  await db.query("UPDATE pricing_settings SET price_mode='inclusive'");await page.goto(url);
};
