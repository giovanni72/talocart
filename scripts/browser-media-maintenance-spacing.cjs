// Shared regression: real maintenance form, localized native controls and geometry.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

module.exports = async ({page, query, origin, name, output}) => {
  const original = (await query('SELECT locale FROM administrators LIMIT 1')).rows[0].locale;
  const originalTheme = (await query('SELECT theme_id FROM storefront_settings')).rows[0].theme_id;
  const results = [];
  try {
    for (const theme of ['default', 'essential']) for (const locale of ['it', 'en']) {
      await query('UPDATE storefront_settings SET theme_id=$1', [theme]);
      await query('UPDATE administrators SET locale=$1', [locale]);
      await page.goto(origin + '/admin/media');
      const form = page.locator('form[action="/admin/media/cache"]');
      await page.getByText(locale === 'it' ? 'Manutenzione immagini' : 'Image maintenance', {exact:true}).click();
      const confirmation = form.locator('label');
      const checkbox = form.getByRole('checkbox');
      const buttons = form.locator('.media-actions button');
      assert.equal(await buttons.count(), 3);
      await checkbox.focus();
      await page.keyboard.press('Space');
      assert.equal(await checkbox.isChecked(), true);
      await page.keyboard.press('Tab');
      assert.equal(await buttons.first().evaluate(e => e === document.activeElement), true);
      const label = await confirmation.boundingBox();
      const geometry = await buttons.evaluateAll(items => items.map(e => {
        const r = e.getBoundingClientRect();
        return {top:r.top, bottom:r.bottom, left:r.left, right:r.right};
      }));
      const gap = Math.min(...geometry.map(r => r.top)) - label.y - label.height;
      assert.ok(gap >= 15.5, `${name}/${locale}: maintenance buttons need 16px above them; actual gap ${gap}px`);
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), 'No page overflow');
      assert.notEqual(await buttons.first().evaluate(e => getComputedStyle(e).outlineStyle), 'none');
      for (let i = 1; i < geometry.length; i++) {
        const previous = geometry[i - 1], current = geometry[i];
        assert.ok(current.left >= previous.right + 4 || current.top >= previous.bottom + 4, 'Wrapped buttons stay separated');
      }
      await form.screenshot({path:path.join(output, `media-maintenance-spacing-${name}-${theme}-${locale}.png`)});
      results.push({name, theme, locale, gap, buttons:geometry, passed:true});
    }
  } finally {
    await query('UPDATE administrators SET locale=$1', [original]);
    await query('UPDATE storefront_settings SET theme_id=$1', [originalTheme]);
  }
  fs.writeFileSync(path.join(output, `media-maintenance-spacing-${name}.json`), JSON.stringify(results, null, 2));
};
