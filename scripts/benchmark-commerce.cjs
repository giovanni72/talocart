// Focused commerce audit; invoked only inside benchmark-site's disposable schema.
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict');
const {randomBytes,createHash}=require('node:crypto');
const {spawnSync}=require('node:child_process');
const {chromium}=require('playwright');
const {expand}=require('./catalog-benchmark-sql.cjs');
module.exports=async({owner,origin,folder,output,binary,cli,measure,start,stop,getLogs,clearLogs,samples,schema,pool})=>{
 const report={fixture_version:3,samples,binary_sha256:createHash('sha256').update(fs.readFileSync(binary)).digest('hex'),reads:[],diagnostics:[],browser:[]};
 const save=()=>fs.writeFileSync(path.join(output,'commerce-results.json'),JSON.stringify(report,null,2));
 await owner.query(`INSERT INTO catalog_option_values(option_id,name) SELECT 1,'Extra '||n FROM generate_series(51,1000) n;
 INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) SELECT 1,ARRAY[id],'HEAVY-'||id,1001,10 FROM catalog_option_values WHERE id>50;
 UPDATE catalog_products SET image_key=NULL; UPDATE catalog_variants SET image_key=NULL; DELETE FROM media_library;`);
 // Deterministic genuine PNGs, no external imaging dependency or application media.
 const media=path.join(folder,'media');fs.mkdirSync(path.join(media,'objects'),{recursive:true});
 const generated=spawnSync('python3',['-c',`import sys,zlib,struct,hashlib,json,pathlib
root=pathlib.Path(sys.argv[1]);out=[]
def chunk(k,v):return struct.pack('!I',len(v))+k+v+struct.pack('!I',zlib.crc32(k+v)&0xffffffff)
for n in range(14):
 w,h=1600,1200
 rows=[bytes(((x//5+y//7+n*17)%256,(x//9+n*31)%256,(y//3+n*13)%256)[c] for x in range(w) for c in range(3)) for y in range(64)]
 raw=b''.join(b'\\0'+rows[y%64] for y in range(h))
 png=b'\\x89PNG\\r\\n\\x1a\\n'+chunk(b'IHDR',struct.pack('!IIBBBBB',w,h,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b'')
 key=hashlib.sha256(png).hexdigest()+'.png';(root/'objects'/key).write_bytes(png);out.append({'key':key,'bytes':len(png),'width':w,'height':h})
print(json.dumps(out))`,media],{encoding:'utf8',timeout:60000});assert.equal(generated.status,0,'Synthetic PNG generation');
 const images=JSON.parse(generated.stdout);for(const image of images)await owner.query('INSERT INTO media_objects(key,content_type,byte_size,width,height,public) VALUES($1,\'image/png\',$2,$3,$4,true)',[image.key,image.bytes,image.width,image.height]);
 await owner.query('UPDATE catalog_products SET image_key=$1 WHERE id=1',[images[0].key]);await owner.query('UPDATE catalog_variants SET image_key=$1 WHERE product_id=1',[images[1].key]);
 for(let i=0;i<12;i++)await owner.query('INSERT INTO catalog_product_images(product_id,image_key,position) VALUES(1,$1,$2)',[images[i+2].key,i+1]);
 await owner.query("INSERT INTO media_library(key,folder_id,name) SELECT key,(SELECT id FROM media_folders WHERE path='library'),key FROM media_objects WHERE key=ANY($1::text[])",[images.map(x=>x.key)]);
 await stop();cli('media-cache');await start('warn');
 report.cache=(await owner.query('SELECT status,count(*)::int n FROM media_cache_jobs GROUP BY status')).rows;
 assert.equal(report.cache.find(x=>x.status==='complete')?.n,14,'Every source must have generated renditions');
 assert.ok(report.cache.every(x=>x.status==='complete'),'No pending/failed cache jobs');
 await owner.query("INSERT INTO checkout_countries(country_code) VALUES('IT') ON CONFLICT DO NOTHING; UPDATE shipping_methods SET active=true,free_from_cents=0,tax_class_id=CASE WHEN id=1 THEN 1 ELSE NULL END; UPDATE shipping_methods SET fee_value=250 WHERE id=1; INSERT INTO shipping_method_countries(method_id,country_code) VALUES(1,'IT'),(2,'IT') ON CONFLICT DO NOTHING; UPDATE payment_methods SET active=(id=2),fee_kind='fixed',fee_value=50,tax_class_id=1 WHERE id IN(1,2); INSERT INTO coupons(code,kind,value,currency,active) VALUES('AUDIT10','percent',1000,'EUR',true);");
 const detailColumns=(await owner.query("SELECT column_name,data_type FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='checkout_details' ORDER BY ordinal_position")).rows;
 const cookies=new Map();
 for(const size of [1,21,84,100]){
  const raw=randomBytes(32).toString('hex'),csrf=randomBytes(32).toString('hex');
  const cart=(await owner.query("INSERT INTO guest_carts(token_hash,csrf) VALUES($1,$2) RETURNING id",[createHash('sha256').update(raw).digest('hex'),csrf])).rows[0].id;
  await owner.query(`INSERT INTO guest_cart_items(cart_id,item_key,product_id,variant_id,had_variant,quantity,name,code,unit_cents,basis_points,price_mode,currency)
   SELECT $1,'fixture-'||p.id,p.id,v.id,v.id IS NOT NULL,1,p.name,coalesce(v.sku,p.model),coalesce(v.price_cents,p.price_cents),2200,s.price_mode,'EUR' FROM catalog_products p JOIN pricing_settings s ON s.id=1 LEFT JOIN LATERAL(SELECT id,sku,price_cents FROM catalog_variants WHERE product_id=p.id ORDER BY id LIMIT 1)v ON true WHERE p.id<=$2`,[cart,size]);
  const details=Object.fromEntries(detailColumns.map(c=>[c.column_name,c.data_type==='boolean'?false:c.data_type==='bigint'?0:'']));
  Object.assign(details,{cart_id:cart,email:'buyer@example.test',first_name:'Alex',last_name:'Example',phone:'+39 3331234567',recipient_first_name:'Alex',recipient_last_name:'Example',shipping_country:'IT',shipping_street:'Via Example',shipping_number:'1',shipping_postal:'00100',shipping_city:'Roma',shipping_region:'RM',customer_type:'private',invoice_channel:'none',billing_same:true});
  await owner.query('INSERT INTO checkout_details SELECT (jsonb_populate_record(NULL::checkout_details,$1::jsonb)).*',[JSON.stringify(details)]);
  await owner.query('UPDATE guest_carts SET coupon_id=1 WHERE id=$1',[cart]);
  cookies.set(size,'talocart_cart='+raw);
 }
 for(const table of ['catalog_products','catalog_variants','catalog_option_values','catalog_variant_values','guest_cart_items','guest_carts'])await owner.query(`VACUUM (ANALYZE) ${schema}.${table}`);
 report.dataset={products:100,variants:Number((await owner.query('SELECT count(*) n FROM catalog_variants')).rows[0].n),heavy_product_variants:1000,gallery:12,source_images:14,coupon_percent:10,shipping_methods:2,payment_methods:1,cart_sizes:[1,21,84,100]};
 if(process.env.TALOCART_CONCURRENCY_BENCH==='1'){
  await require('./benchmark-concurrency.cjs')({owner,origin,output,binary,schema,pool,cookies,dataset:report.dataset});return;
 }
 const source=fs.readFileSync('crates/commerce/src/variants.rs','utf8');
 const axes=source.match(/query\("(SELECT o.id,o.name,[^"]+)", self.language/)[1];
 const old=expand(axes.replace('variant.product_id=$1','variant.product_id=a.product_id'),'it','it');
 const optimized=old.replace('variant.product_id=a.product_id','variant.product_id=$1');
 assert.deepEqual((await owner.query(old,[1,true])).rows,(await owner.query(optimized,[1,true])).rows);
 report.axes_sql={identical:true,before:[],candidate:[]};
 for(let i=0;i<15;i++)for(const [name,sql]of i%2?[['candidate',optimized],['before',old]]:[['before',old],['candidate',optimized]]){
 const plan=(await owner.query('EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) '+sql,[1,true])).rows[0]['QUERY PLAN'][0];report.axes_sql[name].push({planning_ms:plan['Planning Time'],execution_ms:plan['Execution Time'],plan:i===0?plan:undefined});}
 if(process.env.TALOCART_COMMERCE_BENCH_ASSERT_PLAN!=='0'){
 report.axes_owner_plan=(await owner.query('EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) '+expand(axes,'it','it'),[1,true])).rows[0]['QUERY PLAN'][0];
 require('./site-benchmark-contracts.cjs').verifyProductVariantScan(report.axes_owner_plan,1000);
 }
 for(const prefix of ['','/en']){
  for(const route of ['/products/product-1','/products/product-1?value_1=1','/products/product-100'])report.reads.push(await measure('detail',prefix+route,cookies.get(1)));
  for(const size of cookies.keys())for(const route of ['/cart','/checkout','/checkout/methods']){const row=await measure('cart-'+size,prefix+route,cookies.get(size));report.reads.push(row);if(route==='/cart'){const body=fs.readFileSync(path.join(output,'html',row.snapshot),'utf8');assert.equal((body.match(/data-cart-item=/g)||[]).length,size);assert.ok(body.includes('data-cart-total="gross"'));}}
 }
 save();
 await stop();await start('warn,talocart_web=info,sqlx::query=debug');
 for(const [label,route,auth]of [['detail','/products/product-1',cookies.get(1)],['variant','/products/product-1?value_1=1',cookies.get(1)],...[1,21,84,100].flatMap(n=>[['cart-'+n,'/cart',cookies.get(n)],['checkout-'+n,'/checkout',cookies.get(n)],['methods-'+n,'/checkout/methods',cookies.get(n)]])]){
 clearLogs();const requestId=randomBytes(16).toString('hex');const response=await fetch(origin+route,{headers:{cookie:auth,'x-request-id':requestId}});assert.equal(response.status,200);await response.text();await new Promise(r=>setTimeout(r,40));const queries=require('./site-benchmark-contracts.cjs').requestQueries(getLogs(),requestId);assert.ok(queries.length,'Missing request-correlated SQL logs');report.diagnostics.push({label,route,queries:queries.length,summaries:queries.map(x=>({summary:x.fields?.summary,elapsed:x.fields?.elapsed}))});}
 for(const group of ['cart','checkout','methods'])for(const size of [21,84,100])assert.equal(report.diagnostics.find(x=>x.label===group+'-'+size).queries,report.diagnostics.find(x=>x.label===group+'-1').queries,'No per-line SQL growth: '+group);
 save();
 await stop();await start('warn');
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 try{for(const width of [1440,390])for(const js of [true,false])for(const locale of ['it','en']){
  const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});if(js)await context.addInitScript(()=>{
   window.auditPaint={lcp:null,cls:0};
   new PerformanceObserver(list=>{for(const entry of list.getEntries())window.auditPaint.lcp=entry.startTime;}).observe({type:'largest-contentful-paint',buffered:true});
   new PerformanceObserver(list=>{for(const entry of list.getEntries())if(!entry.hadRecentInput)window.auditPaint.cls+=entry.value;}).observe({type:'layout-shift',buffered:true});
  });context.setDefaultTimeout(15000);context.setDefaultNavigationTimeout(30000);const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
  await context.addCookies([{name:'talocart_cart',value:cookies.get(84).split('=')[1],url:origin}]);
  const prefix=locale==='en'?'/en':'',route=prefix+'/products/product-1';const metrics=[];
  for(let n=0;n<3;n++){
   const response=await page.goto(origin+route);assert.equal(response.status(),200);
   await page.locator('.store-detail-photo img').evaluate(img=>img.decode());
   if(js)await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
   metrics.push(await page.evaluate(()=>{const n=performance.getEntriesByType('navigation')[0];return{lcp:window.auditPaint?.lcp??null,cls:window.auditPaint?.cls??null,ttfb:n.responseStart,dom_ready:n.domContentLoadedEventEnd,load:n.loadEventEnd,image_ready:performance.now(),images:performance.getEntriesByType('resource').filter(x=>x.initiatorType==='img').length,image_bytes:performance.getEntriesByType('resource').filter(x=>x.initiatorType==='img').reduce((s,x)=>s+x.transferSize,0),natural_width:document.querySelector('.store-detail-photo img').naturalWidth}}));
  }
  assert.equal(await page.locator('.store-photo-thumbnails a').count(),13);
  const started=performance.now();await page.locator('select[name=value_1]').selectOption('1');if(!js)await page.locator('.store-variant-selector button').click();await page.waitForURL('**/*value_1=1*');await page.locator('[data-product-sku]').filter({hasText:'VAR-1-1'}).waitFor();const variantMs=performance.now()-started;
  await page.locator('.store-photo-thumbnails a').nth(2).click();await page.waitForURL('**/*photo=*');await page.locator('.store-detail-photo img').evaluate(img=>img.decode());
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'No horizontal overflow');
  await page.screenshot({path:path.join(output,`detail-${width}-${js}-${locale}.png`),fullPage:false});
  const cartStart=performance.now();await page.goto(origin+prefix+'/cart');assert.equal(await page.locator('[data-cart-item]').count(),84);const cartMs=performance.now()-cartStart;
  await page.goto(origin+prefix+'/checkout');assert.equal(await page.locator('main').count(),1);assert.deepEqual(errors,[]);
  await page.goto(origin+prefix+'/checkout/methods');await page.locator('#checkout-methods').waitFor();
  const oldRevision=await page.locator('#checkout-methods [name=revision]').inputValue();
  const selected=await page.locator('[name=shipping_method_id]:checked').inputValue();
  const previousTotal=await page.locator('#checkout-summary').innerText();const methodStart=performance.now();
  await page.locator(`[name=shipping_method_id][value="${selected==='1'?'2':'1'}"]`).check();
  if(js)await page.waitForFunction(old=>document.querySelector('#checkout-methods [name=revision]').value!==old,oldRevision);
  else{await page.locator('button[value=update]').click();await page.waitForLoadState('load');}
  assert.notEqual(await page.locator('#checkout-summary').innerText(),previousTotal,'Shipping recalculates totals');
  const methodsMs=performance.now()-methodStart;assert.deepEqual(errors,[]);
  await page.screenshot({path:path.join(output,`checkout-${width}-${js}-${locale}.png`),fullPage:false});
  report.browser.push({width,js,locale,metrics,variant_ms:variantMs,cart_84_navigation_ms:cartMs,shipping_recalculation_ms:methodsMs});save();await context.close();
 }}finally{await browser.close();}
 if(process.env.TALOCART_COMMERCE_BENCH_BASELINE){const baseline=JSON.parse(fs.readFileSync(process.env.TALOCART_COMMERCE_BENCH_BASELINE));report.comparison=require('./site-benchmark-contracts.cjs').compareReports(baseline,report);}
 save();console.log(JSON.stringify({commerce_scenarios:report.reads.length,browser_profiles:report.browser.length,queries:report.diagnostics.map(x=>({label:x.label,queries:x.queries})),output}));
};
