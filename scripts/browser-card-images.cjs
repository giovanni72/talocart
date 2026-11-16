const assert = require('node:assert/strict');
// Fresh contexts prevent cached larger images from masking resource selection.
module.exports = async ({ browser, origin, js = true }) => {
  for (const width of [390, 1440, 1797]) {
    for (const routePath of ['/', '/catalog']) {
      const context = await browser.newContext({viewport:{width,height:1000},deviceScaleFactor:2,javaScriptEnabled:js});
      try {
        await context.route('**/__card-image-*.svg', route => {
          const pixels = Number(new URL(route.request().url()).pathname.match(/(\d+)\.svg$/)[1]);
          return route.fulfill({contentType:'image/svg+xml',body:`<svg xmlns="http://www.w3.org/2000/svg" width="${pixels}" height="${pixels*4/3}"><rect width="100%" height="100%" fill="#47674c"/></svg>`});
        });
        await context.route(origin + routePath, async route => {
          const response = await route.fetch();
          const html = (await response.text()).replace(/(<a class="store-product-photo"[^>]*>\s*)<img[^>]*>/, (match, prefix) => prefix + match.slice(prefix.length).replace(/src="[^"]*"/, 'src="/__card-image-384.svg"').replace(/srcset="[^"]*"/, 'srcset="/__card-image-384.svg 384w, /__card-image-768.svg 768w, /__card-image-1536.svg 1536w"'));
          await route.fulfill({response,body:html});
        });
        const page = await context.newPage();
        await page.goto(origin+routePath);
        const img = page.locator('.store-product-photo img').first();
        await img.scrollIntoViewIfNeeded();
        await img.evaluate(img=>img.decode());
        const result = await img.evaluate(img=>({width:img.getBoundingClientRect().width,dpr:devicePixelRatio,source:img.currentSrc,sizes:img.sizes}));
        const selected = Number(result.source.match(/(\d+)\.svg$/)[1]);
        const adequate = [384,768,1536].find(pixels=>pixels>=result.width*result.dpr) || 1536;
        assert.equal(selected,adequate,`${routePath}, viewport ${width}, JS ${js}: ${JSON.stringify(result)}`);
      } finally { await context.close(); }
    }
  }
};
