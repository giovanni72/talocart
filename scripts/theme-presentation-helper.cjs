// Capture helpers do not change styles. Baseline checks are explicit checkpoints.
const assert = require('node:assert/strict');
const path = require('node:path');
const { verify } = require('./theme-baseline-helper.cjs');
async function screenshot(page, options) {
  if (await page.locator('body[data-theme="essential"]').count()) {
    assert.equal(await page.locator('link[href="/assets/themes/essential.css"]').count(),1);
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'Essential: no horizontal overflow');
    options={...options,path:path.join(path.dirname(options.path),'essential-'+path.basename(options.path))};
  }
  return page.screenshot(options);
}
module.exports={verify,screenshot};
