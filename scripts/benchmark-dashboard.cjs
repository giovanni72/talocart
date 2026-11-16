// Compare two release binaries on the same disposable database and synthetic data.
const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), os = require('node:os');
const {spawn, spawnSync} = require('node:child_process');
const {once} = require('node:events');
const {randomBytes, randomUUID, createHash} = require('node:crypto');
const {Client} = require('pg'), {chromium} = require('playwright');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const stats = values => {
  const sorted = values.slice().sort((a,b) => a-b);
  return {samples:sorted.length, p50_ms:sorted[Math.floor(sorted.length/2)], p95_ms:sorted[Math.ceil(.95*sorted.length)-1]};
};

(async () => {
  assert.ok(process.argv.length===2 || (process.argv.length===3 && process.argv[2]==='--smoke'),'Unknown argument');
  const smoke=process.argv.includes('--smoke');
  assert.ok(process.env.TALOCART_TEST_PREVIOUS_BINARY, 'Previous release binary required');
  const connectionString = require('./checkout-benchmark-contracts.cjs').dedicatedDatabase(process.env.TALOCART_TEST_DATABASE_URL).href;
  const binaries = {before:path.resolve(process.env.TALOCART_TEST_PREVIOUS_BINARY), after:path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart')};
  const output = path.resolve(process.env.TALOCART_TEST_DASHBOARD_OUTPUT || '.local/verification/dashboard-performance');
  fs.mkdirSync(output, {recursive:true});
  const report = {smoke,methodology:smoke?'Tool smoke on 1,000 orders; timings are not performance evidence.':'Same synthetic schema and unchanged data for both release binaries; sequential loopback HTTP, 2 warmups and 9 measured samples; SQL logging in separate passes; browser network includes real filter submission. No WAN or concurrent load.', binaries:{}, reads:[], network:[], diagnostics:[]};
  for (const [label,binary] of Object.entries(binaries)) {
    const identity = spawnSync(binary, ['build-info'], {encoding:'utf8'});
    assert.equal(identity.status,0,'Build identity');
    report.binaries[label] = {identity:JSON.parse(identity.stdout),sha256:createHash('sha256').update(fs.readFileSync(binary)).digest('hex')};
  }
  assert.deepEqual(report.binaries.before.identity.database,report.binaries.after.identity.database,'This comparison requires the same database contract');
  const db = new Client({connectionString}); await db.connect();
  const schema = 'dashboard_bench_'+process.pid+'_'+Date.now();
  const folder = fs.mkdtempSync(path.join(os.tmpdir(),'talocart-dashboard-bench-'));
  const url = new URL(connectionString); url.searchParams.set('options','-csearch_path='+schema);
  let created=false, server, browser, instance=0, logs=[];
  const stop = async () => {
    if (server && server.exitCode===null) { const exited=once(server,'exit');server.kill('SIGTERM');await exited; }
    server=undefined;
  };
  const start = async (label, diagnostic=false) => {
    instance++; const port=3210+instance, origin='http://127.0.0.1:'+port;
    try { await fetch(origin+'/health/ready',{signal:AbortSignal.timeout(500)}); throw Error('Benchmark port occupied'); }
    catch(error) { if(error.message==='Benchmark port occupied')throw error; }
    const config=path.join(folder,'instance-'+instance+'.toml');
    fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:'+port+'"\n',{mode:0o600});
    const env={...Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('TALOCART_'))),TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:'+port,TALOCART_MEDIA_ROOT:path.join(folder,'media-'+instance),TALOCART_LOG:diagnostic?'warn,talocart_web=info,sqlx::query=debug':'warn'};
    logs=[];
    server=spawn(binaries[label],['--config',config,'serve'],{env,stdio:['ignore','pipe','pipe']});
    for(const stream of [server.stdout,server.stderr]) {
      let pending='';stream.on('data',data=>{pending+=data;const lines=pending.split('\n');pending=lines.pop();for(const line of lines){try{logs.push(JSON.parse(line));}catch{}}});
    }
    for(let i=0;i<150;i++) {
      assert.equal(server.exitCode,null,'Temporary server exited');
      try { if((await fetch(origin+'/health/ready')).ok)return origin; } catch {}
      await sleep(100);
    }
    throw Error('Benchmark readiness timeout');
  };
  let cookie;
  const measure = async (origin,label,size,route,partial) => {
    const headers={cookie,...(partial?{'HX-Request':'true'}:{})};
    const times=[];let bytes;
    for(let i=0;i<11;i++) {
      const started=performance.now(),response=await fetch(origin+route,{headers,redirect:'manual'}),body=await response.text();
      const elapsed=performance.now()-started;
      assert.equal(response.status,200);assert.ok(body.includes('id="sales-title"'));
      assert.equal(body.includes('<!doctype html>'),!partial);
      bytes=Buffer.byteLength(body);if(i>=2)times.push(elapsed);
    }
    const row={label,orders:size,route,partial,html_bytes:bytes,...stats(times)};report.reads.push(row);console.log(JSON.stringify(row));
  };
  const network = async (origin,label,size,status) => {
    // Tool smoke can use the same candidate for native and HTMX paths in CI.
    // Real before/after measurements keep JavaScript enabled for both releases.
    const context=await browser.newContext({viewport:{width:1440,height:1000},javaScriptEnabled:!(smoke&&label==='before')});
    await context.addCookies([{name:'talocart_session',value:cookie.slice(cookie.indexOf('=')+1),url:origin}]);
    try {
      const page=await context.newPage(),cdp=await context.newCDPSession(page);await cdp.send('Network.enable');
      let requests=new Map();
      cdp.on('Network.requestWillBeSent',e=>requests.set(e.requestId,{path:new URL(e.request.url).pathname,type:e.type}));
      cdp.on('Network.responseReceivedExtraInfo',e=>{const row=requests.get(e.requestId);if(row)row.status=e.statusCode;});
      cdp.on('Network.loadingFinished',e=>{const row=requests.get(e.requestId);if(row)row.wire_bytes=e.encodedDataLength;});
      await page.goto(origin+'/admin');await page.waitForLoadState('networkidle');requests=new Map();
      const groups=label==='after'?await page.locator('.dashboard-groups').elementHandle():null;
      await page.selectOption('#sales-metric','orders');await page.selectOption('#sales-period','7d');
      await page.locator('.sales-status-filter summary').click();
      for(const input of await page.locator('.sales-status-list input').all())await input.uncheck();
      await page.locator('input[name="status"][value="'+status+'"]').check();
      const response=page.waitForResponse(r=>new URL(r.url()).pathname==='/admin'&&new URL(r.url()).searchParams.get('period')==='7d');
      const started=performance.now();await page.locator('.sales-filters button[type="submit"]').click();await response;
      await page.locator('[data-sales-total][data-currency=""]').waitFor();
      const update_ms=performance.now()-started;await page.waitForLoadState('networkidle');
      const preserved=groups?await groups.evaluate(el=>el.isConnected):false;
      assert.equal(preserved,label==='after');
      const rows=[...requests.values()];if(label==='after')assert.equal(rows.filter(r=>r.path!=='/branding/favicon').length,1);
      report.network.push({label,orders:size,update_ms,cards_preserved:preserved,requests:rows,wire_bytes:rows.reduce((sum,r)=>sum+(r.wire_bytes||0),0)});
    } finally { await context.close(); }
  };
  try {
    await db.query('CREATE SCHEMA '+schema);created=true;await db.query('SET search_path TO '+schema);
    const config=path.join(folder,'migration.toml');fs.writeFileSync(config,'',{mode:0o600});
    const env={...Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('TALOCART_'))),TALOCART_DATABASE_URL:url.href,TALOCART_MEDIA_ROOT:path.join(folder,'migration-media')};
    assert.equal(spawnSync(binaries.before,['--config',config,'migrate'],{env,timeout:120000}).status,0,'Fixture migration');
    const origin=await start('before'),password=randomBytes(24).toString('hex');
    const csrf=html=>html.match(/name="csrf" value="([^"]+)"/)[1];
    const cookieOf=r=>r.headers.get('set-cookie').split(';')[0];
    const post=async(route,cookie,body)=>{const r=await fetch(origin+route,{method:'POST',headers:{cookie},body:new URLSearchParams(body),redirect:'manual'});await r.arrayBuffer();assert.equal(r.status,303);return r;};
    let r=await fetch(origin+'/setup');await post('/setup',cookieOf(r),{csrf:csrf(await r.text()),primary_locale:'it',locale:'it',email:'benchmark@example.test',password,name:'Dashboard benchmark'});
    r=await fetch(origin+'/admin/login');r=await post('/admin/login',cookieOf(r),{csrf:csrf(await r.text()),email:'benchmark@example.test',password});cookie=cookieOf(r);await stop();
    const statuses=(await db.query('SELECT id FROM order_statuses ORDER BY id LIMIT 2')).rows.map(r=>Number(r.id));
    const reference=(await db.query('SELECT statement_timestamp() AS instant')).rows[0].instant;
    report.reference=reference;report.environment={node:process.version,cpu:os.cpus()[0].model,os:os.platform(),postgres:(await db.query('SHOW server_version')).rows[0].server_version};
    await db.query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) SELECT 'Audit '||n,'AUDIT-'||n,'audit-'||n,100,10,'published',(SELECT default_class_id FROM pricing_settings WHERE id=1) FROM generate_series(1,10000) n");
    await db.query("INSERT INTO customers(email,password_hash,first_name,last_name) SELECT 'bench-'||n||'@example.test','synthetic','Synthetic','Customer' FROM generate_series(1,10000) n");
    browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
    let previous=0;
    for(const size of smoke?[1000]:[100000,1000000]) {
      console.log('Preparing synthetic orders:',size);
      await db.query("INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents,created_at) SELECT 'BENCH-'||n,n,0,repeat('a',64),'status_id = '||CASE WHEN n%2=0 THEN $2::bigint ELSE $3::bigint END,'EUR',100,0,0,100,0,100,$4::timestamptz-(n%730)*interval '1 day' FROM generate_series($1::int+1,$5::int) n",[previous,...statuses,reference,size]);
      await db.query("INSERT INTO order_fulfillment(order_id,state) SELECT id,'pending' FROM orders WHERE id>$1",[previous]);previous=size;
      // Startup checks require complete owner projections, including synthetic orders.
      await db.query("INSERT INTO order_customer_summaries(order_id,first_name,last_name,email) SELECT o.id,'Synthetic','Customer','benchmark@example.test' FROM orders o WHERE NOT EXISTS(SELECT 1 FROM order_customer_summaries s WHERE s.order_id=o.id)");
      for(const table of ['orders','order_current_status','order_status_events','order_fulfillment','catalog_products','customers'])await db.query('VACUUM (ANALYZE) '+table);
      // Alternate the version order by size, keeping the data identical within each pair.
      for(const label of size===100000?['before','after']:['after','before']) {
        const origin=await start(label);
        await measure(origin,label,size,'/admin',false);
        for(const route of ['/admin?filtered=1&metric=orders&period=7d&status='+statuses[0],'/admin?period=12m']) {
          await measure(origin,label,size,route,label==='after');
        }
        await network(origin,label,size,statuses[0]);await stop();
      }
    }
    for(const label of ['before','after']) {
      const origin=await start(label,true),id=randomUUID();logs=[];
      const response=await fetch(origin+'/admin?period=7d',{headers:{cookie,'x-request-id':id,...(label==='after'?{'HX-Request':'true'}:{})}});await response.arrayBuffer();assert.equal(response.status,200);await sleep(50);
      const queries=logs.filter(row=>row.target==='sqlx::query'&&row.spans?.some(span=>span.request_id===id));
      assert.ok(queries.length>0,'Request-scoped SQL diagnostics captured');
      report.diagnostics.push({label,queries:queries.length,summaries:queries.map(q=>q.fields?.summary)});await stop();
    }
    report.passed=true;
  } finally {
    await stop();if(browser)await browser.close();if(created)await db.query('DROP SCHEMA '+schema+' CASCADE');await db.end();
    fs.rmSync(folder,{recursive:true,force:true});fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(report,null,2));
  }
})().catch(error=>{console.error(error instanceof assert.AssertionError?error.message:'Dashboard benchmark failed; inspect the dedicated fixture and binary prerequisites');process.exitCode=1;});
