// Opt-in benchmark: dedicated test database, disposable schema and server only.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes, createHash } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { Client } = require('pg');

const { normalizeHtml, compareReports, verifyQueryBudgets, verifyMediaPlan } = require('./site-benchmark-contracts.cjs');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const stats = values => {
  const sorted = [...values].sort((a, b) => a - b);
  const percentile = p => +sorted[Math.ceil(sorted.length * p) - 1].toFixed(2);
  return { samples: values.length, p50_ms: percentile(.5), p95_ms: percentile(.95), max_ms: percentile(1) };
};
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'TALOCART_TEST_DATABASE_URL must identify a dedicated test database');
  const dedicatedURL = require('./checkout-benchmark-contracts.cjs').dedicatedDatabase(process.env.TALOCART_TEST_DATABASE_URL);
  const samples = Number(process.env.TALOCART_SITE_BENCH_SAMPLES || 7);
  assert.ok(Number.isInteger(samples) && samples >= 5 && samples <= 500);
  const pool = Number(process.env.TALOCART_SITE_BENCH_POOL || 5);
  assert.ok(Number.isInteger(pool) && pool >= 1 && pool <= 20);
  const port = Number(process.env.TALOCART_SITE_BENCH_PORT || 3104);
  assert.ok(Number.isInteger(port) && port > 1024 && port <= 65535 && port !== 3000);
  const origin = `http://127.0.0.1:${port}`;
  // Refuse to benchmark an unrelated already-running process.
  try { await fetch(origin + '/health/ready', { signal: AbortSignal.timeout(1000) }); throw new Error('Benchmark port already occupied'); }
  catch (e) { if (e.message === 'Benchmark port already occupied') throw e; }
  const schema = `talocart_bench_${process.pid}_${Date.now()}`;
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-benchmark-'));
  const output = path.resolve(process.env.TALOCART_SITE_BENCH_OUTPUT || (process.env.TALOCART_CHECKOUT_BENCH==='1'?'.local/verification/checkout-performance':process.env.TALOCART_CONCURRENCY_BENCH==='1'?'.local/verification/concurrency-performance':process.env.TALOCART_COMMERCE_BENCH==='1'?'.local/verification/commerce-performance':'.local/verification/site-performance'));
  fs.mkdirSync(output, { recursive: true });
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
  const config = path.join(folder, 'talocart.toml');
  fs.writeFileSync(config, `[server]\nlisten="127.0.0.1:${port}"\n[database]\nmax_connections=${pool}\ntimeout_seconds=3\n`, { mode: 0o600 });
  const url = new URL(dedicatedURL);
  url.searchParams.set('application_name', schema);
  url.searchParams.set('options', `-csearch_path=${schema}`);
  const env = { ...Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('TALOCART_'))), TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: `127.0.0.1:${port}`, TALOCART_MEDIA_ROOT: path.join(folder, 'media'), TALOCART_LOG: 'warn' };
  const owner = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL });
  let server, created = false, logs = [];
  const stop = async () => {
    if (server && server.exitCode === null) { const exited = once(server, 'exit'); server.kill('SIGTERM'); await exited; }
    server = undefined;
  };
  const start = async log => {
    logs = [];
    server = spawn(binary, ['--config', config, 'serve'], { env: { ...env, TALOCART_LOG: log }, stdio: ['ignore', 'pipe', 'pipe'] });
    let pending = '';
    server.stderr.on('data', data => {
      pending += data.toString(); const lines = pending.split('\n'); pending = lines.pop();
      for (const line of lines) { try { logs.push(JSON.parse(line)); } catch {} }
    });
    // Never print diagnostic output: config/SQL may contain synthetic session data.
    server.stdout.resume();
    for (let i = 0; i < 100; i++) {
      assert.equal(server.exitCode, null, 'Benchmark server exited');
      try { if ((await fetch(origin + '/health/ready', { signal: AbortSignal.timeout(1000) })).ok) return; } catch {}
      await sleep(100);
    }
    throw new Error('Benchmark server readiness timeout');
  };
  const cli = (...args) => {
    const result = spawnSync(binary, ['--config', config, ...args], { env, encoding: 'utf8', timeout: 60000 });
    assert.equal(result.status, 0, `CLI ${args[0]} failed (output intentionally omitted)`);
  };
  const post = async (route, cookie, data) => {
    const response = await fetch(origin + route, { method: 'POST', headers: { cookie }, body: new URLSearchParams(data), redirect: 'manual', signal: AbortSignal.timeout(15000) });
    await response.arrayBuffer(); assert.equal(response.status, 303, route); return response;
  };
  const csrf = html => { const match = html.match(/name="csrf" value="([^"]+)"/); assert.ok(match); return match[1]; };
  const cookie = response => response.headers.get('set-cookie').split(';')[0];
  try {
    await owner.connect(); await owner.query(`CREATE SCHEMA ${schema}`); created = true;
    await owner.query(`SET search_path TO ${schema}`); cli('migrate');
    await start('warn');
    const password = randomBytes(24).toString('hex');
    const setup = await fetch(origin + '/setup'); const setupCsrf = csrf(await setup.text());
    await post('/setup', cookie(setup), { csrf: setupCsrf, primary_locale: 'it', locale: 'it', email: 'benchmark@example.test', password, name: 'Benchmark', display_name: 'Benchmark' });
    const login = await fetch(origin + '/admin/login'); const loginCsrf = csrf(await login.text());
    const loggedIn = await post('/admin/login', cookie(login), { csrf: loginCsrf, email: 'benchmark@example.test', password });
    const session = cookie(loggedIn);

    await owner.query(`
      INSERT INTO storefront_languages(code) VALUES ('en') ON CONFLICT DO NOTHING;
      INSERT INTO storefront_language_prefixes(code) VALUES ('en') ON CONFLICT DO NOTHING;
      INSERT INTO catalog_categories(name,slug) SELECT 'Category '||n,'category-'||n FROM generate_series(1,100) n;
      UPDATE catalog_categories SET parent_id=1 WHERE id<>1;
      INSERT INTO catalog_brands(name,slug) SELECT 'Brand '||n,'brand-'||n FROM generate_series(1,100) n;
      INSERT INTO catalog_products(name,model,slug,description,price_cents,stock,status,category_id,brand_id,tax_class_id)
      SELECT 'Product '||n,'AUDIT-'||n,'product-'||n,repeat('Synthetic description. ',50),1000+n,100,'published',n,1,(SELECT default_class_id FROM pricing_settings WHERE id=1) FROM generate_series(1,100) n;
      INSERT INTO catalog_products_translations(entity_id,locale,name,description)
      SELECT id,'en','English '||name,'Translated synthetic description' FROM catalog_products WHERE id%3=0;
      INSERT INTO catalog_categories_translations(entity_id,locale,name)
      SELECT id,'en','English '||name FROM catalog_categories WHERE id%2=0;
      INSERT INTO catalog_brands_translations(entity_id,locale,name) SELECT id,'en','English '||name FROM catalog_brands WHERE id=1;
      INSERT INTO catalog_options(name) VALUES ('Size');
      INSERT INTO catalog_option_values(option_id,name) SELECT id,'Size '||n FROM catalog_options CROSS JOIN generate_series(1,50) n;
      INSERT INTO catalog_product_options(product_id,position,option_id) SELECT p.id,1,o.id FROM catalog_products p CROSS JOIN catalog_options o WHERE p.id<=90;
      INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) SELECT p.id,ARRAY[v.id],'VAR-'||p.id||'-'||v.id,p.price_cents,10 FROM catalog_products p CROSS JOIN catalog_option_values v WHERE p.id<=90;
      INSERT INTO media_objects(key,content_type,byte_size,width,height) SELECT lpad(to_hex(n),64,'0')||'.png','image/png',100,100,100 FROM generate_series(1,1372) n;
      INSERT INTO media_library(key,folder_id,name) SELECT key,(SELECT id FROM media_folders WHERE path='library'),key FROM media_objects;
      UPDATE catalog_products SET image_key=lpad(to_hex(id),64,'0')||'.png';
      UPDATE catalog_variants SET image_key=lpad(to_hex(1+(id%1372)),64,'0')||'.png';
      INSERT INTO information_pages(slug,name,description,published,header_menu) VALUES ('about','About','Synthetic information page',true,true);
      INSERT INTO information_pages_translations(entity_id,locale,name,description) SELECT id,'en','English About','Translated information page' FROM information_pages;
      INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents,created_at)
      SELECT 'AUDIT-'||n,n,0,repeat('a',64),'{}','EUR',1000,0,0,1000,0,1000,'2026-01-01'::timestamptz FROM generate_series(1,10000) n;
      -- Complete the current read projections for synthetic orders before restart checks.
      INSERT INTO order_customer_summaries(order_id,first_name,last_name,email)
      SELECT id,'Synthetic','Customer','customer@example.test' FROM orders;
      INSERT INTO order_current_status(order_id,status_id)
      SELECT o.id,p.initial_order_status_id FROM orders o CROSS JOIN payment_methods p WHERE p.code='bank_transfer';
      INSERT INTO operation_events(area,resource,resource_key,action,created_at) SELECT 'catalog','catalog_products',n::text,'UPDATE','2026-01-01'::timestamptz FROM generate_series(1,100000) n;
      UPDATE media_library SET trashed_at='2026-01-01' WHERE key IN (SELECT key FROM media_objects WHERE id%11=0);
      INSERT INTO media_objects(key,content_type,byte_size,width,height) SELECT lpad(to_hex(n),64,'0')||'.png','image/png',100,100,100 FROM generate_series(2000,2007) n;
      INSERT INTO media_library(key,folder_id,name) SELECT key,(SELECT id FROM media_folders WHERE path='library'),key FROM media_objects WHERE id>1372;
    `);
    const report={generated:new Date().toISOString(),git:spawnSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).stdout.trim(),dataset:{products:100,variants:4500,fixture_version:2,media:1380,orders:10000,additional_operations:100000},reads:[],diagnostics:[]};
    if(process.env.TALOCART_CHECKOUT_BENCH==='1') {
      await require('./benchmark-checkout.cjs')({owner,origin,output,binary,schema,pool,start,stop,getLogs:()=>logs,clearLogs:()=>{logs=[];}});
      return;
    }
    const measure = async (name, route, auth = session, count = samples) => {
      const ttfb = [], times = [];
      let bytes, hash, normalized;
      for (let i = 0; i < count + 2; i++) {
        const begun = performance.now();
        const response = await fetch(origin + route, {headers: {cookie: auth}, redirect: 'manual'});
        const firstByte = performance.now();
        const body = await response.text();
        const complete = performance.now();
        assert.equal(response.status, 200, route);
        if (route.includes('per_page=21') || route.includes('per_page=84')) {
          const size = Number(new URL(route, origin).searchParams.get('per_page'));
          assert.equal((body.match(/class="store-product"/g) || []).length, size, route + ': actual product cards');
        }
        if (i >= 2) {ttfb.push(firstByte - begun); times.push(complete - begun);}
        bytes = Buffer.byteLength(body);
        normalized = normalizeHtml(body, route);
        hash = createHash('sha256').update(normalized).digest('hex');
      }
      const snapshot = createHash('sha256').update(name + ' ' + route).digest('hex').slice(0,16) + '.html';
      fs.mkdirSync(path.join(output, 'html'), {recursive:true});
      fs.writeFileSync(path.join(output, 'html', snapshot), normalized);
      return {name, route, snapshot, ttfb:stats(ttfb), total:stats(times), html_bytes:bytes, html_sha256:hash};
    };
    await owner.query('ALTER TABLE administrators DISABLE TRIGGER operation_event');
    // Variant triggers and fixture image updates leave dead tuples. Clean the
    // disposable schema explicitly so slower baseline routes cannot gain a
    // later autovacuum pass before the storefront scenarios are measured.
    const tables=(await owner.query('SELECT tablename FROM pg_tables WHERE schemaname=$1',[schema])).rows;
    for(const {tablename} of tables) {
      assert.match(tablename,/^\w+$/);
      await owner.query(`VACUUM (ANALYZE) ${schema}.${tablename}`);
    }
    report.fixture_statistics=(await owner.query('SELECT relname,n_live_tup,n_dead_tup FROM pg_stat_user_tables WHERE schemaname=$1 ORDER BY relname',[schema])).rows;
    if(process.env.TALOCART_COMMERCE_BENCH==='1') {
      await require('./benchmark-commerce.cjs')({owner,origin,folder,output,binary,cli,measure,start,stop,getLogs:()=>logs,clearLogs:()=>{logs=[];},samples,schema,pool});
      return;
    }
    const routes=['/admin','/admin/catalog/products?per_page=20','/admin/catalog/products?per_page=100','/admin/catalog/products?sort=price&per_page=100','/admin/catalog/products/1','/admin/catalog/products/1/variants?per_page=20','/admin/catalog/products/1/variants?per_page=100&combination=Size','/admin/catalog/categories','/admin/catalog/brands','/admin/catalog/options','/admin/media','/admin/media?usage=used','/admin/media?usage=unused','/admin/media?trash=true','/admin/media?sort=newest','/admin/media?sort=size','/admin/media?sort=dimensions','/admin/orders','/admin/operations','/admin/operations?area=catalog','/admin/pages','/admin/banners','/admin/settings','/admin/settings/email','/admin/settings/shipping','/admin/settings/payments','/admin/settings/order-statuses','/admin/settings/coupons','/admin/modules','/admin/users','/admin/roles','/admin/account'];
    for(const locale of ['it','en']) {await owner.query('UPDATE administrators SET locale=$1',[locale]);for(const route of routes){const r=await measure(locale,route);report.reads.push(r);console.log(JSON.stringify(r))}}
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(report,null,2));
    let guest='';
    const cart=await fetch(origin+'/cart');guest=cart.headers.getSetCookie().map(x=>x.split(';')[0]).join('; ');await cart.text();const cartHtml=await (await fetch(origin+'/products/product-100',{headers:{cookie:guest}})).text();
    await post('/cart/add',guest,{csrf:csrf(cartHtml),revision:'0',product_id:'100',variant_id:'',quantity:'1'});
    for(const prefix of ['','/en'])for(const route of ['/','/catalog?per_page=21','/catalog?per_page=84','/brands/brand-1?per_page=21','/brands/brand-1?per_page=84','/catalog/category-1?per_page=21','/catalog/category-1?per_page=84','/search/Product?per_page=21','/search/Product?per_page=84','/products/product-1','/products/product-100','/pages/about','/cart','/checkout','/account/login','/account/register']) {const r=await measure('public fixture',(prefix && route==='/'?prefix:prefix+route),guest);report.reads.push(r);console.log(JSON.stringify(r))}
    const registration=await fetch(origin+'/account/register',{headers:{cookie:guest}});const authCookies=registration.headers.getSetCookie().map(x=>x.split(';')[0]).join('; ');const regBody=await registration.text();
    const registered=await post('/account/register',guest+'; '+authCookies,{csrf:csrf(regBody),email:'customer@example.test',first_name:'Synthetic',last_name:'Customer',password,password_confirmation:password,next:'/account'});
    const customer=guest+'; '+registered.headers.getSetCookie().map(x=>x.split(';')[0]).join('; ');
    for(const prefix of ['','/en'])for(const route of ['/cart','/checkout','/account','/account/addresses','/account/orders','/account/password','/account/email']){const r=await measure('customer fixture',prefix+route,customer);report.reads.push(r);console.log(JSON.stringify(r))}
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(report,null,2));
    report.environment={node:process.version,os:os.platform()+' '+os.release(),arch:os.arch(),cpu:os.cpus()[0].model,postgres:(await owner.query('SHOW server_version')).rows[0].server_version,pool};
    report.methodology='Release loopback HTTP; synthetic disposable PostgreSQL schema; EN/IT, sequential samples after two warmups, VACUUM ANALYZE of fixture tables before timing. SQL logs only in separate diagnostic pass. No WAN, images, browser paint or concurrency simulation. Hashes normalize CSRF, the email-test idempotency token, settings MVCC revision and saved-clock preview only; fixture event timestamps are fixed. Business revisions, rows, prices and counts remain compared.';
    report.binary_sha256=createHash('sha256').update(fs.readFileSync(binary)).digest('hex');
    // Query counts in a separate pass, with debug logging excluded from the timings above.
    await stop();await start('warn,sqlx::query=debug');
    for(const route of ['/admin','/admin/catalog/products/1','/admin/media?usage=used','/admin/settings','/cart','/checkout','/account']) {logs=[];const r=await fetch(origin+route,{headers:{cookie:route.startsWith('/admin')?session:customer},redirect:'manual'});await r.arrayBuffer();await sleep(50);const q=logs.filter(x=>x.target==='sqlx::query');const row={route,status:r.status,queries:q.length,summaries:q.map(x=>({summary:x.fields?.summary,elapsed:x.fields?.elapsed}))};report.diagnostics.push(row);console.log(JSON.stringify({route,queries:q.length}));}
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(report,null,2));
    // Owner projections expanded with the same locale rules as the Rust module.
    // Capture planning separately: fewer translation joins primarily help the planner.
    const {constant,expand}=require('./catalog-benchmark-sql.cjs');
    report.translation_plans=[];
    await owner.query('BEGIN READ ONLY');
    try {
      for(const locale of ['it','en'])for(const [name,sql,values] of [
        ['admin_products_21',constant('SELECT')+' ORDER BY lower(p.name),p.id LIMIT 21',[]],
        ['admin_products_84',constant('SELECT')+' ORDER BY lower(p.name),p.id LIMIT 84',[]],
        ['product_detail',constant('PUBLIC_SELECT')+' WHERE p.id=$1',[1]],
        ['homepage',constant('PUBLIC_CARD_SELECT')+' ORDER BY lower(p.name),p.id LIMIT 8',[]],
      ]) {
        const query=expand(sql,locale,'it');
        const plan=(await owner.query('EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) '+query,values)).rows[0]['QUERY PLAN'][0];
        report.translation_plans.push({name,locale,translation_joins:(query.match(/_translations /g)||[]).length,plan});
      }
    } finally {await owner.query('ROLLBACK');}
    if(process.env.TALOCART_SITE_BENCH_ASSERT_QUERIES!=='0') {
      verifyQueryBudgets(report);
      const source=fs.readFileSync('crates/platform/src/media/library.rs','utf8');
      const columns=source.match(/const ASSET_LIST_SELECT: &str = "([^"\n]+)";/)?.[1];
      const predicate=source.match(/"\{ASSET_LIST_SELECT\} (WHERE [^"\n]+)"/)?.[1];
      assert.ok(columns && predicate,'Missing owner media list projection');
      await owner.query('BEGIN READ ONLY');
      try {
        // A stable loop budget; parallel workers would multiply per-worker loop counts.
        await owner.query('SET LOCAL max_parallel_workers_per_gather=0');
        report.media_plan=(await owner.query('EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) SELECT count(*) FROM ('+columns+' '+predicate+') counted',[null,null,'','',false,'used',false])).rows[0]['QUERY PLAN'][0];
      } finally {await owner.query('ROLLBACK');}
      verifyMediaPlan(report.media_plan);
    }
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(report,null,2));
    if(process.env.TALOCART_SITE_BENCH_BASELINE)report.comparison=compareReports(JSON.parse(fs.readFileSync(process.env.TALOCART_SITE_BENCH_BASELINE)),report);
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(report,null,2));
  } finally { await stop();if(created)await owner.query(`DROP SCHEMA ${schema} CASCADE`);await owner.end();fs.rmSync(folder,{recursive:true,force:true}); }
})().catch(e=>{console.error({kind:e.name,code:e.code,column:e.column,position:e.position,message:e.code && e.code!=='ERR_ASSERTION'?(e.code==='42703'&&/^column [A-Za-z0-9_.\" ]{1,120} does not exist$/.test(e.message)?e.message:'SQL error'):e.message});process.exitCode=1});
