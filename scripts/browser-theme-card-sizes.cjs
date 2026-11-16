// Compare rendered Askama `sizes` with the actual CSS layout, without a database.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const { chromium } = require('playwright');
const presentation = require('./theme-presentation-helper.cjs');
(async () => {
  const samples = JSON.parse(fs.readFileSync(0, 'utf8'));
  const css = ['assets/base.css', 'themes/default/assets/store.css', 'themes/essential/assets/store.css'].map(file => fs.readFileSync(file, 'utf8')).join('\n');
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {});
  let checked = 0;
  try {
    const page = await browser.newPage();
    for (const width of [390, 500, 501, 800, 1000, 1001, 1440, 1920]) {
      await page.setViewportSize({ width, height: 1000 });
      for (const { compact, nested, sizes } of samples) {
        const cards = '<article class="store-product"><a class="store-product-photo"><img alt=""></a></article>'.repeat(6);
        const grid = `<div class="store-products">${cards}</div>`;
        await page.setContent(`<style>${css}</style><body class="store-page" data-theme="essential" data-density="${compact ? 'compact' : 'comfortable'}"><div class="store-shell">${nested ? `<section class="home-featured">${grid}</section>` : `<main class="store-catalog"><div class="catalog-layout"><aside></aside><section class="catalog-results">${grid}</section></div></main>`}</div></body>`);
        const measured = await page.evaluate(sizes => {
          const clauses = sizes.split(/,\s*(?=\(max-width:|calc\()/);
          let expression;
          for (const clause of clauses) {
            const condition = clause.match(/^(\(max-width: [^)]+\))\s*(.*)$/);
            if (!condition || matchMedia(condition[1]).matches) { expression = condition ? condition[2] : clause; break; }
          }
          const probe = document.createElement('div');
          probe.style.cssText = `position:fixed;visibility:hidden;width:${expression};height:0`;
          document.body.append(probe);
          const declared = probe.getBoundingClientRect().width;
          probe.remove();
          return { actual: document.querySelector('.store-product-photo img').getBoundingClientRect().width, declared };
        }, sizes);
        assert.ok(Math.abs(measured.actual - measured.declared) < 1, JSON.stringify({ width, compact, nested, ...measured }));
        checked++;
      }
    }
    // A capture works without the external stylesheets required by the oracle,
    // and must leave the document unchanged. The CSS check is a separate action.
    await page.setContent('<main><h1>Capture contract</h1></main>');
    const before = await page.content();
    const capture = await presentation.screenshot(page, {});
    assert.ok(capture.length > 0);
    assert.equal(await page.content(), before);
    assert.equal(await page.locator('#theme-regression-baseline').count(), 0);
    // Reproduce navigation before the shared stylesheet has finished loading.
    await page.route('http://theme-audit.test/**', async route => {
      if (route.request().url().endsWith('/style.css')) {
        await new Promise(resolve => setTimeout(resolve, 200));
        await route.fulfill({ contentType: 'text/css', body: css + fs.readFileSync('assets/ui.css', 'utf8') });
      } else {
        await route.fulfill({ contentType: 'text/html', body: '<link rel="stylesheet" href="/style.css"><main><button class="ui-button">Save</button></main>' });
      }
    });
    await page.goto('http://theme-audit.test/', { waitUntil: 'commit' });
    await page.locator('button').waitFor({ state: 'attached' });
    await require('./ui-contract.cjs')(page);
  } finally { await browser.close(); }
  console.log(JSON.stringify({ checked, passed: true }));
})().catch(error => { console.error(error); process.exitCode = 1; });
