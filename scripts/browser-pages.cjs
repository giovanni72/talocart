const presentation = require('./theme-presentation-helper.cjs');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');

(async () => {
  const { origin, admin } = JSON.parse(fs.readFileSync(0, 'utf8'));
  const out = path.resolve('.local/verification/pages/browser');
  fs.mkdirSync(out, { recursive: true });
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {});
  const results = [];
  try {
    for (const [profile, width, js] of [['desktop', 1440, true], ['mobile', 390, true], ['no-js', 1440, false], ['mobile-no-js', 390, false]]) {
      for (const locale of ['en', 'it']) {
        const context = await browser.newContext({ viewport: { width, height: 1000 }, javaScriptEnabled: js });
        await context.addCookies([{ name: 'talocart_session', value: admin, url: origin }]);
        try {
          const page = await context.newPage();
          const errors = [];
          page.on('pageerror', e => errors.push(e.message));
          const shot = async name => {
            assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), 'No horizontal overflow');
            { const shot = { path: path.join(out, `${profile}-${locale}-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
          };
          const save = async status => {
            const response = page.waitForResponse(r => r.request().method() === 'POST' && r.url().includes('/admin/pages/'));
            await page.locator('button[value=save]').click();
            assert.equal((await response).status(), status);
            await page.waitForLoadState('load');
          };
          await page.goto(origin + '/admin/account');
          await page.locator('#account-language').selectOption(locale);
          await page.locator('form[action="/admin/account/language"] button').click();
          await page.waitForLoadState('load');
          await page.goto(origin + '/admin/pages/new');
          assert.equal(await page.locator('html').getAttribute('lang'), locale);
          assert.ok((await page.locator('h1').innerText()).includes(locale === 'it' ? 'Pagine informative' : 'Information pages'));
          const slug = `page-${profile}-${locale}`;
          const title = `About the shop ${profile} ${locale}`;
          const italian = `Il nostro negozio ${profile} ${locale}`;
          await page.locator('#page-name').fill(title);
          await page.locator('#page-name').focus();
          await page.keyboard.press('Tab');
          assert.equal(await page.locator(js ? '#page-description-tab-visual' : '#page-description-format').evaluate(e => e === document.activeElement), true);
          if (js) {
            await page.locator('#page-description-tab-html').click();
            await page.locator('#page-description-html').fill('<p>First paragraph.</p><p>&lt;script&gt;window.__pagesInjected = true&lt;/script&gt;<br>Text &amp; details.</p>');
          } else await page.locator('#page-description').fill('First paragraph.\n\n<script>window.__pagesInjected = true</script>\nText & details.');
          await page.locator('#page-seo_title').fill('About & details');
          await page.locator('#page-seo_description').fill('Useful shop information.');
          await page.locator('#page-content-it-name').fill(italian);
          if (js) {
            await page.locator('#page-content-it-description-tab-html').click();
            await page.locator('#page-content-it-description-html').fill('<p>Primo paragrafo.</p><p>Consegna, contatti e dettagli.</p>');
          } else await page.locator('#page-content-it-description').fill('Primo paragrafo.\n\nConsegna, contatti e dettagli.');
          await page.locator('#page-content-it-seo_title').fill('Negozio & dettagli');
          await page.locator('#page-content-it-seo_description').fill('Informazioni utili sul negozio.');
          await page.locator('#page-slug').fill('invalid/slug');
          await page.locator('#page-published').check();
          await page.locator('#page-header').check();
          await page.locator('#page-footer').check();
          await save(422);
          assert.equal(await page.locator('#page-slug').getAttribute('aria-invalid'), 'true');
          assert.equal(await page.locator('#page-content-it-name').inputValue(), italian);
          assert.equal(await page.locator('#page-name').inputValue(), title);
          await shot('validation');
          await page.locator('#page-slug').fill(slug);
          await save(303);
          await page.waitForURL(/\/admin\/pages\/\d+\?saved=1$/);
          const editor = page.url();
          await shot('editor');
          const prefix = locale === 'it' ? '/it' : '';
          await page.goto(origin + prefix + '/pages/' + slug);
          assert.equal(await page.locator('html').getAttribute('lang'), locale);
          assert.equal(await page.locator('h1').innerText(), locale === 'it' ? italian : title);
          assert.equal(await page.locator('.information-body p').count(), 2);
          const homeBox = await page.locator('.breadcrumb a').boundingBox();
          const currentBox = await page.locator('.breadcrumb [aria-current=page]').boundingBox();
          assert.ok(currentBox.x >= homeBox.x + homeBox.width + 8 || currentBox.y > homeBox.y + 2, 'Breadcrumb items need visible separation');
          assert.equal(await page.evaluate(() => window.__pagesInjected), undefined);
          assert.equal(await page.locator('link[rel=canonical]').getAttribute('href'), origin + prefix + '/pages/' + slug);
          assert.equal(await page.locator('link[hreflang=en]').count(), 1);
          const header = page.locator('.information-header a[href$="/pages/' + slug + '"]');
          assert.equal(await header.getAttribute('aria-current'), 'page');
          await header.focus();
          assert.equal(await header.evaluate(e => e === document.activeElement), true);
          await shot('public');
          const other = locale === 'en' ? 'it' : 'en';
          await page.locator('.store-languages a[lang="' + other + '"]').click();
          await page.waitForLoadState('load');
          assert.equal(await page.locator('h1').innerText(), other === 'it' ? italian : title);
          await page.goto(origin + prefix + '/catalog');
          assert.equal(await page.locator('.information-header a[href$="/pages/' + slug + '"]').count(), 1);
          assert.equal(await page.locator('.site-footer a[href$="/pages/' + slug + '"]').count(), 1);
          await shot('menu');
          await page.goto(editor);
          await page.locator('#page-slug').fill(slug + '-new');
          await save(303);
          const redirect = await context.request.get(origin + prefix + '/pages/' + slug, { maxRedirects: 0 });
          assert.equal(redirect.status(), 301);
          assert.equal(redirect.headers().location, prefix + '/pages/' + slug + '-new');
          await page.locator('#page-header').uncheck();
          await save(303);
          await page.goto(origin + prefix + '/pages/' + slug + '-new');
          assert.equal(await page.locator('.information-header a[href$="/pages/' + slug + '-new"]').count(), 0);
          assert.equal(await page.locator('.site-footer a[href$="/pages/' + slug + '-new"]').count(), 1);
          await page.goto(editor);
          await page.locator('#page-footer').uncheck();
          await page.locator('#page-header').check();
          await save(303);
          await page.goto(origin + prefix + '/pages/' + slug + '-new');
          assert.equal(await page.locator('.information-header a[href$="/pages/' + slug + '-new"]').count(), 1);
          assert.equal(await page.locator('.site-footer a[href$="/pages/' + slug + '-new"]').count(), 0);
          await page.goto(editor);
          await page.locator('#page-published').uncheck();
          await save(303);
          assert.equal((await context.request.get(origin + prefix + '/pages/' + slug)).status(), 404);
          await page.goto(origin + prefix + '/catalog');
          assert.equal(await page.locator('a[href$="/pages/' + slug + '-new"]').count(), 0);
          await page.goto(editor);
          // A second editor advances the revision while the first keeps its delete form.
          const concurrent = await context.newPage();
          await concurrent.goto(editor);
          await concurrent.locator('#page-name').fill(title + ' updated');
          await concurrent.locator('button[value=save]').click();
          await concurrent.waitForURL(/\/admin\/pages\/\d+\?saved=1$/);
          await concurrent.close();
          await page.locator('.information-delete summary').click();
          const conflict = page.waitForResponse(r => r.request().method() === 'POST' && r.url().includes('/admin/pages/'));
          await page.locator('button[value=delete]').click();
          assert.equal((await conflict).status(), 409);
          await page.waitForLoadState('load');
          assert.equal(await page.locator('#page-name').count(), 0);
          assert.ok((await page.locator('#pages-error').innerText()).includes(locale === 'it' ? 'La cancellazione non è stata eseguita' : 'This deletion was not performed'));
          assert.ok((await page.locator('.catalog-list').innerText()).includes(title + ' updated'));
          await shot('delete-conflict');
          await page.locator('.catalog-row h2 a[href="' + new URL(editor).pathname + '"]').click();
          await page.locator('.information-delete summary').click();
          await page.locator('button[value=delete]').click();
          await page.waitForURL(/\/admin\/pages\?saved=1$/);
          await shot('list');
          assert.deepEqual(errors, []);
          results.push({ profile, locale, passed: true });
        } finally { await context.close(); }
      }
    }
  } finally { await browser.close(); }
  fs.writeFileSync(path.join(out, 'results.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results));
})().catch(e => { console.error(e); process.exitCode = 1; });
