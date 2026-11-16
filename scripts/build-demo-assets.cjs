// Development-only raster export of the repository's own SVG illustrations.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
(async () => {
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {});
  try {
    const page = await browser.newPage({ viewport: { width: 512, height: 512 }, deviceScaleFactor: 1 });
    const folder = path.resolve('seeders/catalog-demo/assets');
    for (const name of fs.readdirSync(folder).filter(name => name.endsWith('.svg')).sort()) {
      await page.setContent(`<style>body{margin:0}svg{display:block}</style>${fs.readFileSync(path.join(folder, name), 'utf8')}`);
      await page.locator('svg').screenshot({ path: path.join(folder, name.replace(/\.svg$/, '.png')), omitBackground: true });
    }
  } finally { await browser.close(); }
})().catch(error => { console.error(error.message); process.exitCode = 1; });
