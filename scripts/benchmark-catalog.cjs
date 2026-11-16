// Opt-in benchmark: dedicated test database, disposable schema and server only.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes, createHash } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { Client } = require('pg');
const { chromium } = require('playwright');
const { constant, query, expand: localizedSql, compareReports } = require('./catalog-benchmark-sql.cjs');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const stats = values => {
  const sorted = [...values].sort((a, b) => a - b);
  const percentile = p => +sorted[Math.ceil(sorted.length * p) - 1].toFixed(2);
  return { samples: values.length, p50_ms: percentile(.5), p95_ms: percentile(.95), max_ms: percentile(1) };
};
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'TALOCART_TEST_DATABASE_URL must identify a dedicated test database');
  // Resolve every diagnostic owner before spending time on the measured workload.
  const sqlInputs = {
    scope: constant('PUBLIC_CATEGORY_SCOPE'), select: constant('PUBLIC_CARD_SELECT'), predicate: constant('PUBLIC_FILTER'),
    count: query('SELECT count(*) FROM [[visible_products]] p JOIN [[products]] source'),
    brands: query('SELECT name,slug FROM [[brands]] WHERE active'),
    categoryBrands: query('{PUBLIC_CATEGORY_SCOPE}, relevant AS'),
    categories: query('WITH RECURSIVE relevant_categories(id,parent_id)'),
  };
  const ownerQuery = sql => localizedSql(sql.replaceAll('{PUBLIC_CATEGORY_SCOPE}',sqlInputs.scope).replaceAll('{PUBLIC_FILTER}',sqlInputs.predicate),'it','it');
  const samples = Number(process.env.TALOCART_BENCH_SAMPLES || 50);
  assert.ok(Number.isInteger(samples) && samples >= 5 && samples <= 500);
  const port = Number(process.env.TALOCART_BENCH_PORT || 3103);
  assert.ok(Number.isInteger(port) && port > 1024 && port <= 65535 && port !== 3000);
  const origin = `http://127.0.0.1:${port}`;
  // Refuse to benchmark an unrelated already-running process.
  try { await fetch(origin + '/health/ready', { signal: AbortSignal.timeout(1000) }); throw new Error('Benchmark port already occupied'); }
  catch (e) { if (e.message === 'Benchmark port already occupied') throw e; }
  const schema = `talocart_bench_${process.pid}_${Date.now()}`;
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-benchmark-'));
  const output = path.resolve(process.env.TALOCART_BENCH_OUTPUT || '.local/verification/catalog-performance');
  fs.mkdirSync(output, { recursive: true });
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
  const config = path.join(folder, 'talocart.toml');
  fs.writeFileSync(config, `[server]\nlisten="127.0.0.1:${port}"\n[database]\nmax_connections=5\ntimeout_seconds=3\n`, { mode: 0o600 });
  const url = new URL(process.env.TALOCART_TEST_DATABASE_URL);
  url.searchParams.set('options', `-csearch_path=${schema}`);
  const env = { ...process.env, TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: `127.0.0.1:${port}`, TALOCART_MEDIA_ROOT: path.join(folder, 'media'), TALOCART_LOG: 'warn' };
  const owner = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL });
  let server, browser, created = false, logs = [];
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
  const get = async (route, ttfb) => {
    const begun = performance.now();
    const response = await fetch(origin + route, { signal: AbortSignal.timeout(15000) });
    if (ttfb) ttfb.push(performance.now()-begun);
    assert.equal(response.status, 200, route); return response.text();
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
    const admin = await fetch(origin + '/admin', { headers: { cookie: session } }); const adminCsrf = csrf(await admin.text());
    // Isolated fixture only: keep the first pass genuinely free of auto-analyzed stats.
    const tables = (await owner.query('SELECT tablename FROM pg_tables WHERE schemaname=$1', [schema])).rows;
    for (const { tablename } of tables) { assert.match(tablename, /^\w+$/); await owner.query(`ALTER TABLE ${schema}.${tablename} SET (autovacuum_enabled=false)`); }
    await owner.query(`
      INSERT INTO storefront_languages(code) VALUES ('en') ON CONFLICT DO NOTHING;
      INSERT INTO storefront_language_prefixes(code) VALUES ('en') ON CONFLICT DO NOTHING;
      INSERT INTO catalog_categories(name,slug) SELECT 'Category '||lpad(n::text,3,'0'),'category-'||n FROM generate_series(1,100) n;
      UPDATE catalog_categories SET parent_id=1 WHERE id<>1;
      INSERT INTO catalog_brands(name,slug) SELECT 'Brand '||lpad(n::text,4,'0'),'brand-'||n FROM generate_series(1,1001) n;
      INSERT INTO catalog_products(name,model,slug,description,price_cents,stock,status,category_id,brand_id,tax_class_id)
      SELECT 'Product '||lpad(n::text,5,'0'),'BENCH-'||n,'product-'||n,repeat('Synthetic description. ',10),1000+n,
        CASE WHEN n%7=0 THEN 0 ELSE 10 END,CASE WHEN n%10=0 THEN 'draft' ELSE 'published' END,
        1+(n-1)%100,CASE WHEN n%5=1 THEN 1 ELSE 1+(n-1)%1001 END,(SELECT default_class_id FROM pricing_settings WHERE id=1) FROM generate_series(1,10000) n;
      INSERT INTO catalog_products_translations(entity_id,locale,name,description)
      SELECT id,'en','English '||name,'Translated synthetic description' FROM catalog_products WHERE id%3=0;
      INSERT INTO catalog_options(name) VALUES ('Size');
      INSERT INTO catalog_option_values(option_id,name) SELECT id,v FROM catalog_options CROSS JOIN (VALUES ('S'),('M')) x(v);
      INSERT INTO catalog_product_options(product_id,position,option_id) SELECT p.id,1,o.id FROM catalog_products p CROSS JOIN catalog_options o WHERE p.id%5=1;
      INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock)
      SELECT p.id,ARRAY[v.id],'VAR-'||p.id||'-'||v.id,p.price_cents+v.id,CASE WHEN p.id%7=0 THEN 0 ELSE 5 END
      FROM catalog_products p CROSS JOIN catalog_option_values v WHERE p.id%5=1;
    `);
    const count = Number((await owner.query('SELECT count(*) FROM catalog_visible_products')).rows[0].count); assert.equal(count, 9000);
    const report = {
      generated_at: new Date().toISOString(),
      environment: { os: `${os.platform()} ${os.release()}`, arch: os.arch(), cpu: os.cpus()[0].model, logical_cpus: os.cpus().length, ram_gib: +(os.totalmem()/2**30).toFixed(1), node: process.version, postgres: (await owner.query('SHOW server_version')).rows[0].server_version, pool: 5, binary: path.basename(binary), binary_sha256: createHash('sha256').update(fs.readFileSync(binary)).digest('hex'), git: spawnSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).stdout.trim() },
      dataset: { fixture_version: 2, products: 10000, published: count, variants: 4000, categories: 100, category_depth: 2, brands: 1001, images: 0 },
      methodology: 'Loopback HTTP, release binary, sequential reads, body fully consumed, timings exclude SQL debug logging. reads_unanalyzed has no explicit warmups; reads has 5 warmups per route after ANALYZE. These phases concern planner statistics, not a flushed OS/browser cache. Write percentiles exclude revision preparation; throughput includes it. Concurrent writes use one owner/session and distinct products; global and session/business locks contribute. No WAN or image decoding simulation.',
      reads: {}, reads_unanalyzed: {}, writes: {},
    };
    const scenarios = { home: '/', catalog: '/catalog', deep_page: '/catalog?page=400', literal_search: '/search/Product%20099', variant_search: '/search/VAR-9991', category_parent: '/catalog/category-1', filters: '/catalog/category-1?brand=brand-1&available=true', category_search: '/search/Product?category=category-1', category_brand: '/brands/brand-1?category=category-1', selected_brand: '/catalog?brand=brand-1001', detail: '/products/product-1', sitemap: '/sitemap-products.xml?page=9' };
    // Both languages and page sizes on the same synthetic data, with enough rows
    // in the brand/search/category to actually return 84 cards.
    const listingScenarios = {};
    for (const [language, prefix] of [['it', ''], ['en', '/en']]) {
      for (const [kind, path] of Object.entries({catalog:'/catalog',brand:'/brands/brand-1',category:'/catalog/category-1',search:'/search/Product'})) {
        for (const size of [21,84]) listingScenarios[`${kind}_${language}_${size}`] = `${prefix}${path}?per_page=${size}`;
      }
    }
    Object.assign(scenarios, listingScenarios);
    const snapshots = {};
    const htmlHash = body => createHash('sha256').update(body).digest('hex');
    const diagnosticPlans = async () => {
      const plans = {};
      for (const language of ['it','en']) for (const size of [21,84]) {
        const sql = localizedSql(`${constant('PUBLIC_CATEGORY_SCOPE')} ${constant('PUBLIC_CARD_SELECT')} WHERE ${constant('PUBLIC_FILTER')} ORDER BY lower(p.name),p.id LIMIT $6 OFFSET $5`, language, 'it');
        const result = (await owner.query('EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) '+sql,['','','',false,0,size])).rows[0]['QUERY PLAN'][0];
        const singletonScans = node => {
          if (['pricing_settings','store_settings'].includes(node['Relation Name'])) assert.equal(node['Plan Rows'],1,'Singleton scans must use their unique key, even without statistics');
          for (const child of node.Plans || []) singletonScans(child);
        };
        singletonScans(result.Plan);
        plans[`${language}_${size}`] = result;
      }
      return plans;
    };
    // Validate every extracted count/selector query before collecting costly samples.
    for (const [sql,params] of [[sqlInputs.count,['','','',false]], [sqlInputs.brands,['brand-1001']],
      [sqlInputs.categoryBrands,['','category-1']], [sqlInputs.categories,['']]]) {
      await owner.query('EXPLAIN (FORMAT JSON) '+ownerQuery(sql),params);
    }
    report.plans_unanalyzed = await diagnosticPlans();
    report.statistics_before = (await owner.query("SELECT relname,reltuples FROM pg_class WHERE relnamespace=$1::regnamespace AND relname IN ('pricing_settings','store_settings','catalog_products','catalog_variants') ORDER BY relname",[schema])).rows;
    for (const [name,route] of Object.entries(listingScenarios)) {
      const times=[],ttfb=[]; let body;
      for (let i=0;i<samples;i++) { const t=performance.now(); body=await get(route,ttfb); times.push(performance.now()-t); }
      snapshots[name]=htmlHash(body);
      const expected = name.endsWith('_84') ? 84 : 21;
      assert.equal((body.match(/class="store-product"/g)||[]).length,expected,`${name}: actual card count`);
      report.reads_unanalyzed[name]={route,...stats(times),ttfb:stats(ttfb),html_bytes:Buffer.byteLength(body),html_sha256:snapshots[name],cards:expected};
    }
    // Only this disposable schema. Compare the exact same pages before/after ANALYZE.
    for (const { tablename } of tables) await owner.query(`ANALYZE ${schema}.${tablename}`);
    report.plans_analyzed = await diagnosticPlans();
    const rss = () => {
      const result = spawnSync('ps', ['-o', 'rss=', '-p', String(server.pid)], { encoding: 'utf8' });
      assert.equal(result.status, 0); return +(Number(result.stdout.trim())/1024).toFixed(2);
    };
    report.rss_idle_mib = rss(); let peakRss = report.rss_idle_mib;
    for (const [name, route] of Object.entries(scenarios)) {
      for (let i=0; i<5; i++) await get(route);
      const times = [], ttfb = []; let body;
      for (let i=0; i<samples; i++) { const t=performance.now(); body=await get(route,ttfb); times.push(performance.now()-t); peakRss=Math.max(peakRss,rss()); }
      if (snapshots[name]) assert.equal(htmlHash(body),snapshots[name],`${name}: statistics must not change HTML`);
      report.reads[name] = { route, ...stats(times), ttfb:stats(ttfb), html_bytes: Buffer.byteLength(body), html_sha256:htmlHash(body) };
    }
    if (process.env.TALOCART_BENCH_BASELINE) {
      const baseline=JSON.parse(fs.readFileSync(process.env.TALOCART_BENCH_BASELINE,'utf8'));
      report.comparison=compareReports(baseline,report,Object.keys(listingScenarios));
    }

    const revisions=new Map();
    const refreshRevisions=async()=>{
      for (const row of (await owner.query('SELECT id,stock_revision FROM catalog_products WHERE id<=8')).rows) revisions.set(Number(row.id),row.stock_revision);
    };
    const write = async n => post(`/admin/catalog/products/${n}`, session, { csrf: adminCsrf, stock_revision: revisions.get(n), name: `Product ${String(n).padStart(5,'0')}`, model: `BENCH-${n}`, slug: `product-${n}`, description: 'Synthetic benchmark update', price: '10.00', price_mode: 'inclusive', stock: '10', status: 'published', category_id: '1', brand_id: '1', tax_class_id: '1' });
    for (const concurrency of [1,4,8]) {
      const times = []; const begun = performance.now();
      for (let batch=0; batch<Math.ceil(samples/concurrency); batch++) {
        await refreshRevisions(); // Edit snapshot preparation is outside the timed POST.
        await Promise.all(Array.from({length:concurrency}, async (_,i) => {const t=performance.now(); await write(i+1); times.push(performance.now()-t);}));
      }
      report.writes[concurrency] = {...stats(times), requests_per_second: +(times.length*1000/(performance.now()-begun)).toFixed(2)};
      peakRss=Math.max(peakRss,rss());
    }
    // Prove the lock is an actual wait, rather than infer contention from elapsed HTTP time.
    await refreshRevisions();
    await owner.query('BEGIN'); await owner.query('SELECT id FROM identity_state WHERE id=1 FOR UPDATE');
    const blocker = (await owner.query('SELECT pg_backend_pid() AS pid')).rows[0].pid;
    const started = performance.now(); const waitingWrite = write(2);
    // Attach a rejection handler immediately while independently observing PostgreSQL locks.
    waitingWrite.catch(() => {});
    let observed = false;
    try {
      for (let i=0; i<100; i++) {
        observed = (await owner.query("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND wait_event_type='Lock') AS waiting", [blocker])).rows[0].waiting;
        if (observed) break; await sleep(10);
      }
      assert.ok(observed, 'Product write must wait for identity authorization lock');
      const t = performance.now(); await get('/'); report.read_during_write_lock_ms = +(performance.now()-t).toFixed(2);
      await sleep(200); // Controlled delay for attribution, excluded from organic write percentiles.
    } finally { await owner.query('ROLLBACK'); await waitingWrite; }
    report.controlled_lock = { observed, delay_ms: 200, write_ms: +(performance.now()-started).toFixed(2) };
    report.rss_sampled_peak_mib = peakRss;
    browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? {channel:process.env.TALOCART_BROWSER_CHANNEL} : {});
    const page = await browser.newPage({ viewport: {width:390,height:844} });
    const errors = []; page.on('pageerror', e => errors.push(e.message));
    const assets = new Map();
    for (const route of ['/', '/catalog', '/products/product-1']) {
      await page.goto(origin+route, {waitUntil:'networkidle'});
      // Resource Timing also works for revalidated/cached assets; a 304 has no
      // network body for Playwright's response.body() to retrieve.
      for (const asset of await page.evaluate(()=>performance.getEntriesByType('resource')
        .filter(r=>/\.(css|js)$/.test(new URL(r.name).pathname))
        .map(r=>({path:new URL(r.name).pathname,type:r.name.includes('.css')?'stylesheet':'script',bytes:r.encodedBodySize,transfer_bytes:r.transferSize})))) {
        assets.set(asset.path,asset);
      }

      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `Mobile overflow ${route}`);
    }
    assert.deepEqual(errors, []);
    report.assets = [...assets.values()];
    await page.screenshot({path:path.join(output,'mobile-product.png'),fullPage:true});
    report.mobile = { viewport:'390x844', pages:3, overflow:false, javascript_errors:0, browser:browser.version(), network:'local, unthrottled' };
    report.listing_browser = [];
    for (const javaScriptEnabled of [true,false]) for (const viewport of [{width:1440,height:900},{width:390,height:844}]) {
      const context = await browser.newContext({viewport,javaScriptEnabled});
      const listingPage = await context.newPage();
      const pageErrors=[]; listingPage.on('pageerror', error=>pageErrors.push(error.message));
      for (const [name,route] of Object.entries(listingScenarios)) {
        await listingPage.goto(origin+route,{waitUntil:'networkidle'});
        const measured=await listingPage.evaluate(()=>({cards:document.querySelectorAll('.store-product').length,overflow:document.documentElement.scrollWidth>innerWidth,navigation:performance.getEntriesByType('navigation')[0].toJSON(),resources:performance.getEntriesByType('resource').map(r=>({name:new URL(r.name).pathname,transfer:r.transferSize,encoded:r.encodedBodySize}))}));
        assert.equal(measured.cards,name.endsWith('_84')?84:21);
        assert.equal(measured.overflow,false,`${name}: browser overflow`);
        report.listing_browser.push({name,javaScriptEnabled,viewport,...measured});
      }
      assert.deepEqual(pageErrors,[]);
      await context.close();
    }
    const assetResponse=await page.request.get(origin+'/assets/base.css');
    const etag=assetResponse.headers().etag; assert.ok(etag,'Static asset validator');
    const cachedAsset=await page.request.get(origin+'/assets/base.css',{headers:{'if-none-match':etag}});
    assert.equal(cachedAsset.status(),304); assert.equal((await cachedAsset.body()).length,0);
    report.asset_revalidation={status:cachedAsset.status(),body_bytes:0};
    await browser.close(); browser=undefined;
    // Preserve completed measurements if a later diagnostic fails; results.json remains the success record.
    fs.writeFileSync(path.join(output,'measurements.json'),JSON.stringify({status:'measurements_complete_diagnostics_pending',...report},null,2)+'\n');
    // Separate diagnostic pass: count actual SQLx statement events, without skewing timing samples.
    await stop(); await start('warn,sqlx::query=debug'); await sleep(150);
    for (const [name,route] of Object.entries(scenarios)) {
      logs=[]; await get(route); await sleep(100);
      const queries=logs.filter(l => l.target==='sqlx::query');
      assert.ok(queries.length>0, 'SQL statement diagnostics must be enabled');
      report.reads[name].sql_statements=queries.length;
      report.reads[name].sql_rows_returned=queries.map(l => ({summary:l.fields.summary, rows:l.fields.rows_returned}));
    }
    // EXPLAIN real catalog query extracted from the Rust source to avoid a drifting SQL copy.
    const expand = sql => localizedSql(sql, 'it', 'it');
    const {scope,select,predicate} = sqlInputs;
    const filtered = expand(`${scope} ${select} WHERE ${predicate} ORDER BY lower(p.name),p.id LIMIT $6 OFFSET $5`);
    const plans={};
    for (const [name,params] of Object.entries({catalog:['','','',false,0,21],deep_page:['','','',false,8379,21],literal_search:['Product 099','','',false,0,21],selected_brand:['','','brand-1001',false,0,21],category_parent:['','category-1','',false,0,21],category_filtered:['','category-1','brand-1',true,0,21]})) {
      plans[name]=(await owner.query('EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) '+filtered,params)).rows[0]['QUERY PLAN'];
    }
    const countSql=ownerQuery(sqlInputs.count);
    plans.catalog_count=(await owner.query('EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) '+countSql,['','','',false])).rows[0]['QUERY PLAN'];
    plans.filtered_count=(await owner.query('EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) '+countSql,['Product 099','','',true])).rows[0]['QUERY PLAN'];
    plans.category_count=(await owner.query('EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) '+countSql,['','category-1','',false])).rows[0]['QUERY PLAN'];
    assert.equal(plans.category_count[0].Plan.Plans.find(p => p['Subplan Name'] === 'CTE category_scope')['Actual Rows'], 100);
    plans.brand_selector=(await owner.query('EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) '+ownerQuery(sqlInputs.brands),['brand-1001'])).rows[0]['QUERY PLAN'];
    plans.category_brands=(await owner.query('EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) '+ownerQuery(sqlInputs.categoryBrands),['','category-1'])).rows[0]['QUERY PLAN'];
    const categorySql=ownerQuery(sqlInputs.categories);
    plans.search_categories=(await owner.query('EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) '+categorySql,[''])).rows[0]['QUERY PLAN'];
    plans.brand_categories=(await owner.query('EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) '+categorySql,['brand-1'])).rows[0]['QUERY PLAN'];
    fs.writeFileSync(path.join(output,'plans.json'),JSON.stringify(plans,null,2)+'\n');
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(report,null,2)+'\n');
    console.log(JSON.stringify({output,reads:report.reads,writes:report.writes,rss_mib:peakRss,lock:report.controlled_lock},null,2));
  } finally {
    if (browser) await browser.close(); await stop();
    if (created) { await owner.query('ROLLBACK'); await owner.query(`DROP SCHEMA ${schema} CASCADE`); }
    await owner.end(); fs.rmSync(folder,{recursive:true,force:true});
  }
})().catch(error => { console.error(error instanceof assert.AssertionError ? error.message : {failure:error.name,code:error.code,frames:error.stack?.split('\n').slice(1,5)}); process.exitCode=1; });
