// Called with the dashboard suite's synthetic owner and isolated PostgreSQL schema.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

module.exports = async ({ page, origin, output, theme, locale, profile }) => {
  const initial = page.viewportSize();
  const widths = profile.startsWith('mobile') ? [800, 650, 390, 320] : [2560, 1920, 1440, 1280];
  const routes = [
    ['/admin/customer-groups/1', '#cg-name_it'],
    ['/admin/pages/new', profile.includes('no-js') ? '#page-description' : '#page-description-visual'],
    ['/admin/banners/new', '#banner-label'],
    ['/admin/catalog/categories/new', 'input[name="name"]'],
    ['/admin/roles/new', 'input[name="name"]'],
    ['/admin/settings/customer-fields', null],
    ['/admin/components', '#example-label'],
  ];
  const measurements = [];
  try {
    for (const width of widths) {
      await page.setViewportSize({ width, height: 1000 });
      for (const [route, control] of routes) {
        const response = await page.goto(origin + route);
        assert.equal(response.status(), 200, `${route}: ${locale} ${width}`);
        assert.equal(await page.locator('html').getAttribute('lang'), locale);
        const allCards = page.locator('.editor-card');
        const measured = new Map();
        const measure = async () => {
          const visible = await allCards.evaluateAll(cards => cards.flatMap((card,index) => {
            if (!card.getClientRects().length) return [];
            const parent = card.parentElement;
            const style = getComputedStyle(parent);
            return [{index, id:card.id, width:card.getBoundingClientRect().width,
              available:parent.clientWidth-parseFloat(style.paddingLeft)-parseFloat(style.paddingRight)}];
          }));
          assert.ok(visible.length, `${route}: visible editor cards present`);
          for (const card of visible) {
            assert.ok(Math.abs(card.width-card.available)<2,
              `${route} #${card.id}: editor uses available width at ${width}: ${card.width}/${card.available}`);
            measured.set(card.index,card);
          }
        };
        if (route === '/admin/components' && !profile.includes('no-js')) {
          const tabs = page.locator('[data-ui-tabs-ready] [role=tab]');
          assert.equal(await tabs.count(),5,'All example panels remain covered');
          await tabs.first().focus(); await tabs.first().press('Enter');
          for (let index=0;index<await tabs.count();index++) {
            if (index) await page.keyboard.press('ArrowRight');
            const tab = tabs.nth(index);
            assert.equal(await tab.getAttribute('aria-selected'),'true');
            assert.equal(await tab.evaluate(e=>e===document.activeElement),true);
            const panel = page.locator('#'+await tab.getAttribute('aria-controls'));
            assert.equal(await panel.isVisible(),true,'Selected panel is visible before measuring');
            await measure();
          }
        } else await measure();
        assert.equal(measured.size,await allCards.count(),`${route}: every editor card measured while visible`);
        const cards = [...measured.values()];
        if (control) {
          const field = page.locator(control).first();
          assert.equal(await field.count(), 1, `${route}: representative field`);
          const fieldWidth = (await field.boundingBox()).width;
          const fraction = route.includes('/catalog/') || route === '/admin/components' ? .3 : .65;
          if (width >= 1280) assert.ok(fieldWidth > cards[0].width * fraction,
            `${route}: fields grow with card (${fieldWidth}/${cards[0].width})`);
          await field.focus();
          assert.equal(await field.evaluate(el => el === document.activeElement), true);
          assert.notEqual(await field.evaluate(el => getComputedStyle(el).outlineStyle), 'none');
        }
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
          `${route}: no horizontal page overflow at ${width}`);
        measurements.push({ width, route, cards });
        if ([2560, 390].includes(width)) await page.screenshot({
          path: path.join(output, `${theme}-${locale}-${profile}-${width}-${route.split('/')[2]}.png`), fullPage: true,
        });
      }
    }
    // A native validation error must preserve the draft and the fluid form without saving.
    await page.goto(origin + '/admin/pages/new');
    await page.locator('#page-name').fill('Layout draft');
    await page.locator('#page-slug').fill('invalid slug');
    await page.locator('form[action="/admin/pages/new"] button[type="submit"]').first().click();
    assert.equal(await page.locator('#page-name').inputValue(), 'Layout draft');
    assert.equal(await page.locator('#page-slug').getAttribute('aria-invalid'), 'true');
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    fs.writeFileSync(path.join(output, `${theme}-${locale}-${profile}-widths.json`), JSON.stringify(measurements, null, 2));
  } finally {
    await page.setViewportSize(initial);
  }
};
