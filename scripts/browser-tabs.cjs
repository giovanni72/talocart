// Source-level browser regressions, also run by the demo component checks in CI.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');
const source = name => fs.readFileSync(path.join(__dirname, '..', 'assets', name), 'utf8');
module.exports = async browser => {
  const failures = [];
  for (const language of ['it', 'en']) for (const variant of ['', 'ui-tabs--line']) {
    const context = await browser.newContext({ viewport: { width: 900, height: 500 }, reducedMotion: 'reduce' });
    const page = await context.newPage();
    try {
      const labels = language === 'it' ? ['Generali', 'Prezzo e disponibilità', 'Organizzazione', 'Attributi'] : ['General', 'Price and availability', 'Organisation', 'Attributes'];
      await page.setContent(`<style>:root{--ink:#123;--white:white;--paper:#eee;--line:#ccc;--body:Arial}*{box-sizing:border-box}body{margin:0}#host{width:300px}</style><style>${source('ui.css')}</style><div id="host"><nav class="ui-tabs ${variant}"><button type="button" aria-label="Previous" tabindex="-1" class="ui-button ui-button--secondary ui-button--icon ui-button--compact ui-tabs__scroll ui-tabs__scroll--start"></button><div class="ui-tabs__list">${labels.map((label, i) => `<a class="ui-tabs__tab" href="#panel-${i}" ${i === 3 ? 'aria-current="page"' : ''}>${label}</a>`).join('')}</div><button type="button" aria-label="Next" tabindex="-1" class="ui-button ui-button--secondary ui-button--icon ui-button--compact ui-tabs__scroll ui-tabs__scroll--end"></button></nav></div>`);
      assert.equal(await page.locator('.ui-tabs__scroll--end').isVisible(), false, 'Native fallback hides scroll controls');
      await page.addScriptTag({ content: source('ui-tabs.js') });
      await page.waitForTimeout(100);
      const lastVisible = await page.locator('.ui-tabs__list').evaluate(list => list.lastElementChild.getBoundingClientRect().right <= list.getBoundingClientRect().right + 1);
      if (!lastVisible) failures.push(`${language}/${variant || 'rail'}: active last tab clipped on initial load`);
      await page.locator('.ui-tabs__scroll--start').click();
      await page.waitForTimeout(100);
      assert.ok(await page.locator('.ui-tabs__list').evaluate(list => list.scrollLeft < list.scrollWidth - list.clientWidth - 1), 'Previous button scrolls away from active tab');
      // Content fits without buttons, but not with them: widening must remove both.
      const width = await page.locator('.ui-tabs__list').evaluate(list => list.scrollWidth + 25);
      await page.locator('#host').evaluate((host, width) => { host.style.width = `${width}px`; }, width);
      await page.waitForTimeout(100);
      if (await page.locator('.ui-tabs__scroll--end').isVisible()) failures.push(`${language}/${variant || 'rail'}: controls remain after content fits`);
      await page.locator('#host').evaluate(host => { host.style.width = '300px'; });
      await page.waitForTimeout(100);
      assert.equal(await page.locator('.ui-tabs__scroll--end').isVisible(), true, 'Controls return after shrinking');
    } finally { await context.close(); }
  }
  assert.deepEqual(failures, [], 'Tab overflow regressions');
};
if (require.main === module) (async () => {
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {});
  try { await module.exports(browser); console.log('Tab overflow regressions passed (EN/IT, rail/line, initial load, buttons, resize, native fallback).'); }
  finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
