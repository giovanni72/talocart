const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page, output, name, context}) => {
  const nav = page.locator('.shared-pagination');
  assert.equal(await nav.count(), 1);
  assert.match(await nav.locator('.pagination-summary').innerText(), /Prodotti [\d–]+ di \d+\nPagina \d+ di \d+/);
  assert.equal(await nav.locator('.pagination-button').count(), 2);
  for (const button of await nav.locator('.pagination-button').all()) {
    const box = await button.boundingBox();
    assert.ok(box.height >= 44 && box.width >= 100, 'Large navigation targets');
    const state = await button.evaluate(e => ({tag:e.tagName, disabled:e.getAttribute('aria-disabled'),bg:getComputedStyle(e).backgroundColor,color:getComputedStyle(e).color,weight:getComputedStyle(e).fontWeight}));
    if (state.tag === 'A') {
      assert.notEqual(state.bg, 'rgba(0, 0, 0, 0)');
      assert.notEqual(state.bg, state.color);
      assert.ok(Number(state.weight) >= 600);
      await button.focus();
      assert.ok(await button.evaluate(e => e === document.activeElement && getComputedStyle(e).outlineStyle !== 'none'));
    } else assert.equal(state.disabled, 'true');
  }
  const total = (await nav.locator('.pagination-summary').innerText()).match(/di (\d+)/)[1];
  const count = page.locator('.catalog-heading .catalog-result-count');
  if (context === 'catalog') assert.equal(await count.innerText(), `${total} ${total === '1' ? 'prodotto' : 'prodotti'}`);
  const original = page.viewportSize();
  for (const width of [320, 390, 768, 1440]) {
    await page.setViewportSize({width,height:900});
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `No overflow at ${width}`);
    if (context === 'catalog') {
    const countBox = await count.boundingBox();
    const headingBox = await page.locator('.catalog-heading').boundingBox();
    assert.ok(Math.abs(countBox.x - headingBox.x) < 2 && countBox.width + countBox.x <= width, 'Total aligned left and within viewport');
    if ([320,1440].includes(width)) await page.locator('.catalog-heading').screenshot({path:path.join(output,`product-total-${name}-${width}.png`)});
    }
    const buttons = await nav.locator('.pagination-button').evaluateAll(es => es.map(e => {const r=e.getBoundingClientRect();return {left:r.left,right:r.right};}));
    assert.ok(buttons.every(b => b.left >= 0 && b.right <= width), 'Navigation fits viewport');
    if ([320,1440].includes(width)) await nav.screenshot({path:path.join(output,`shared-pagination-${context}-${name}-${width}.png`)});
  }
  await page.setViewportSize(original);
};
