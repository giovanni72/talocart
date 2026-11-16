// Read-only HTTP load against benchmark-commerce's disposable synthetic fixture.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { randomBytes, createHash } = require('node:crypto');
const { normalizeHtml } = require('./site-benchmark-contracts.cjs');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const hash = body => createHash('sha256').update(normalizeHtml(body)).digest('hex');
const stats = values => {
  const sorted = [...values].sort((a,b)=>a-b);
  const at = p => +sorted[Math.ceil(sorted.length*p)-1].toFixed(2);
  return {samples:values.length,p50_ms:at(.5),p95_ms:at(.95),max_ms:at(1)};
};
module.exports = async ({owner,origin,output,binary,schema,pool,cookies,dataset}) => {
  const iterations = Number(process.env.TALOCART_CONCURRENCY_ITERATIONS || 10);
  assert.ok(Number.isInteger(iterations) && iterations >= 3 && iterations <= 100);
  const levels = [1,5,10,20];
  const report = {fixture_version:1,dataset,pool,iterations,levels,
    binary_sha256:createHash('sha256').update(fs.readFileSync(binary)).digest('hex'),
    model:'Closed-loop: each worker sends the next GET after consuming the previous body; no think time. PostgreSQL activity sampled every 20ms; no SQL text or session tokens saved.',runs:[]};
  report.environment={node:process.version,os:os.platform()+' '+os.release(),arch:os.arch(),cpu:os.cpus()[0].model,postgres:(await owner.query('SHOW server_version')).rows[0].server_version};
  const save = () => fs.writeFileSync(path.join(output,'concurrency-results.json'),JSON.stringify(report,null,2));
  // Clone the 100-line cart into independent sessions; defaults create fresh IDs/timestamps.
  const sourceToken = createHash('sha256').update(cookies.get(100).split('=')[1]).digest('hex');
  const source = (await owner.query('SELECT id FROM guest_carts WHERE token_hash=$1',[sourceToken])).rows[0].id;
  const independent = [];
  for(let i=0;i<20;i++) {
    const raw = randomBytes(32).toString('hex');
    const id = (await owner.query('INSERT INTO guest_carts(token_hash,csrf,coupon_id) SELECT $1,$2,coupon_id FROM guest_carts WHERE id=$3 RETURNING id',
      [createHash('sha256').update(raw).digest('hex'),randomBytes(32).toString('hex'),source])).rows[0].id;
    await owner.query(`INSERT INTO guest_cart_items(cart_id,item_key,product_id,variant_id,had_variant,quantity,name,code,unit_cents,basis_points,price_mode,currency)
      SELECT $1,item_key,product_id,variant_id,had_variant,quantity,name,code,unit_cents,basis_points,price_mode,currency FROM guest_cart_items WHERE cart_id=$2`,[id,source]);
    await owner.query("INSERT INTO checkout_details SELECT (jsonb_populate_record(NULL::checkout_details,to_jsonb(d)||jsonb_build_object('cart_id',$1::bigint))).* FROM checkout_details d WHERE cart_id=$2",[id,source]);
    independent.push('talocart_cart='+raw);
  }
  for(const table of ['guest_carts','guest_cart_items','checkout_details'])await owner.query(`VACUUM (ANALYZE) ${schema}.${table}`);
  for(const prefix of ['','/en'])for(const [name,route,shared] of [
    ['catalog-21','/catalog?per_page=21',false],['catalog-84','/catalog?per_page=84',false],
    ['detail','/products/product-1',false],['cart-100','/cart',false],
    ['checkout-100','/checkout',false],['methods-100','/checkout/methods',false],
    ['methods-shared-100','/checkout/methods',true],
  ]) {
    const fullRoute=prefix+route;
    const request=async worker=>{
      const began=performance.now();
      const response=await fetch(origin+fullRoute,{headers:{cookie:independent[shared?0:worker]},redirect:'manual',signal:AbortSignal.timeout(15000)});
      const ttfb=performance.now()-began,body=await response.text(),total=performance.now()-began;
      assert.equal(response.status,200,fullRoute);
      if(name.startsWith('catalog-'))assert.equal((body.match(/class="store-product"/g)||[]).length,Number(name.split('-')[1]),'Actual product card count');
      if(name==='cart-100')assert.equal((body.match(/data-cart-item=/g)||[]).length,100,'Actual cart line count');
      return {ttfb,total,html:hash(body)};
    };
    const baseline=[];
    for(let worker=0;worker<20;worker++) {await request(worker);baseline.push((await request(worker)).html);}
    for(const concurrency of levels) {
      const times=[],ttfb=[],waits={samples:0,blocked_samples:0,max_blocked_connections:0,max_active_connections:0,max_connections:0,events:{}};
      let done=false;
      const monitor=(async()=>{
        while(!done) {
          const rows=(await owner.query(`SELECT state,wait_event_type,wait_event,cardinality(pg_blocking_pids(pid)) blockers
            FROM pg_stat_activity WHERE datname=current_database() AND application_name=$1`,[schema])).rows;
          waits.samples++;
          waits.max_connections=Math.max(waits.max_connections,rows.length);
          const blocked=rows.filter(x=>x.blockers>0).length;
          if(blocked)waits.blocked_samples++;
          waits.max_blocked_connections=Math.max(waits.max_blocked_connections,blocked);
          waits.max_active_connections=Math.max(waits.max_active_connections,rows.filter(x=>x.state==='active').length);
          for(const row of rows)if(row.state==='active' && row.wait_event_type) {
            const key=row.wait_event_type+':'+row.wait_event;waits.events[key]=(waits.events[key]||0)+1;
          }
          await sleep(20);
        }
      })();
      const began=performance.now();
      let elapsed;
      try {
        const results=await Promise.allSettled(Array.from({length:concurrency},(_,worker)=>(async()=>{
          for(let n=0;n<iterations;n++) {
            const result=await request(worker);
            assert.equal(result.html,baseline[worker],fullRoute+' response must match sequential reference');
            times.push(result.total);ttfb.push(result.ttfb);
          }
        })()));
        elapsed=performance.now()-began;
        for(const result of results)if(result.status==='rejected')throw result.reason;
      } finally {done=true;await monitor;}

      const row={name,locale:prefix?'en':'it',route:fullRoute,concurrency,requests:times.length,total:stats(times),ttfb:stats(ttfb),requests_per_second:+(times.length*1000/elapsed).toFixed(2),identical_html:true,waits};
      assert.ok(waits.max_connections>0,'Monitor must observe the benchmark application connections');
      assert.ok(waits.max_connections<=pool,'Temporary server must respect the configured pool bound');
      report.runs.push(row);save();console.log(JSON.stringify({name,locale:row.locale,concurrency,p95:row.total.p95_ms,blocked:waits.blocked_samples}));
    }
  }
  assert.equal(report.runs.length,56);save();
};
