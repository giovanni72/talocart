// Synthetic PNGs exercise the real upload, card/picker and detail pages, including no-JS.
const assert = require('node:assert/strict');
const path = require('node:path');
const {execFileSync} = require('node:child_process');

module.exports = async ({page, query, origin, name, output}) => {
  const originalLocale = (await query('SELECT locale FROM administrators LIMIT 1')).rows[0].locale;
  const originalTheme = (await query('SELECT theme_id FROM storefront_settings')).rows[0].theme_id;
  const fixtures = JSON.parse(execFileSync('python3', ['-c', `import base64,json,struct,zlib
def chunk(kind,data):return struct.pack('!I',len(data))+kind+data+struct.pack('!I',zlib.crc32(kind+data)&0xffffffff)
out=[]
for name,w,h in [('square',50,50),('wide',100,25),('tall',25,100),('large-wide',1200,300),('large-tall',300,1200)]:
 raw=(b'\\0'+bytes([32,112,144])*w)*h
 png=b'\\x89PNG\\r\\n\\x1a\\n'+chunk(b'IHDR',struct.pack('!IIBBBBB',w,h,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b'')
 out.append(dict(name=name,width=w,height=h,data=base64.b64encode(png).decode()))
print(json.dumps(out))`], {encoding:'utf8'}));
  const prefix = `preview-${name}-`;
  const geometry = async locator => {
    // Native lazy loading also applies with JavaScript disabled; enter the viewport first.
    await locator.scrollIntoViewIfNeeded();
    return locator.evaluate(async img => {
    await img.decode();
    const r = img.getBoundingClientRect(), style = getComputedStyle(img);
    const frame = img.parentElement.getBoundingClientRect(), parentStyle = getComputedStyle(img.parentElement);
    return {
      width:r.width-parseFloat(style.paddingLeft)-parseFloat(style.paddingRight),
      height:r.height-parseFloat(style.paddingTop)-parseFloat(style.paddingBottom),
      nativeWidth:img.naturalWidth, nativeHeight:img.naturalHeight,
      availableWidth:frame.width-parseFloat(parentStyle.paddingLeft)-parseFloat(parentStyle.paddingRight)-parseFloat(parentStyle.borderLeftWidth)-parseFloat(parentStyle.borderRightWidth),
      frameWidth:frame.width, frameHeight:frame.height,
    };
    });
  };
  const checkSize = (g, context) => {
    assert.ok(g.width <= g.nativeWidth + .5 && g.height <= g.nativeHeight + .5,
      `${context}: native ${g.nativeWidth}x${g.nativeHeight}, displayed ${g.width}x${g.height}; previews must never upscale`);
    assert.ok(Math.abs(g.width/g.height-g.nativeWidth/g.nativeHeight) < .02, `${context}: preserve aspect ratio`);
    assert.ok(g.width <= g.availableWidth + 1, `${context}: fit the available width`);
  };
  try {
    await query("UPDATE administrators SET locale='it'");
    await page.goto(origin + '/admin/media');
    await page.locator('summary').filter({hasText:/^Carica file$/}).click();
    await page.getByLabel('File da caricare', {exact:true}).setInputFiles(fixtures.map(f => ({name:prefix+f.name+'.png', mimeType:'image/png', buffer:Buffer.from(f.data,'base64')})));
    await page.getByRole('button', {name:'Carica nella mediateca',exact:true}).click();
    await page.waitForURL(url => url.pathname === '/admin/media' && url.searchParams.has('selected'));
    const images = (await query('SELECT m.id,l.name FROM media_library l JOIN media_objects m ON m.key=l.key WHERE left(l.name,length($1))=$1 ORDER BY m.width*m.height,l.name', [prefix])).rows;
    assert.equal(images.length, fixtures.length);
    const square = images.find(image => image.name === prefix+'square.png');
    for (const theme of ['default','essential']) for (const locale of ['it','en']) {
      await query('UPDATE storefront_settings SET theme_id=$1', [theme]);
      await query('UPDATE administrators SET locale=$1', [locale]);
      // Reproduce the reported 50px detail before checking the other orientations.
      await page.goto(origin + `/admin/media/${square.id}`);
      const small = await geometry(page.locator('.media-detail-preview img'));
      checkSize(small, `${name}/${theme}/${locale}/50px detail`);
      assert.equal(small.width, 50); assert.equal(small.height, 50);
      await page.screenshot({path:path.join(output, `media-sizing-detail-${name}-${theme}-${locale}.png`),fullPage:true});
      for (const image of images.filter(image => image.id !== square.id)) {
        await page.goto(origin + `/admin/media/${image.id}`);
        const g = await geometry(page.locator('.media-detail-preview img'));
        checkSize(g, `${name}/${theme}/${locale}/${image.name}`);
        assert.ok(g.height <= 650.5, 'Large detail previews stay bounded');
      }
      for (const target of ['', 'logo']) {
        await page.goto(origin + '/admin/media?q=' + encodeURIComponent(prefix) + '&target=' + target);
        assert.equal(await page.locator('.media-card').count(), fixtures.length);
        for (const image of images) {
          const g = await geometry(page.locator(`[data-media-id="${image.id}"] .media-preview img`));
          checkSize(g, `${name}/${theme}/${locale}/card/${image.name}`);
          assert.ok(Math.abs(g.frameHeight-g.frameWidth*.75) < 1, 'Tall images must not expand the 4:3 card frame');
        }
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
      }
      await page.screenshot({path:path.join(output, `media-sizing-grid-${name}-${theme}-${locale}.png`),fullPage:true});
    }
  } finally {
    await query('UPDATE administrators SET locale=$1', [originalLocale]);
    await query('UPDATE storefront_settings SET theme_id=$1', [originalTheme]);
  }
};
