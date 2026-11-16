// Invoked only by benchmark-site with its disposable synthetic schema and server.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { randomBytes, createHash } = require('node:crypto');
const { spawnSync } = require('node:child_process');
const { stats, confirmationFields, technicalFailures } = require('./checkout-benchmark-contracts.cjs');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
module.exports = async ({owner, origin, output, binary, schema, pool, start, stop, getLogs, clearLogs}) => {
  const rounds = 3, levels = [1,5,10,20];
  const identity = spawnSync(binary,['build-info'],{encoding:'utf8',timeout:30000});
  assert.equal(identity.status,0,'Build identity required');
  const report = {fixture_version:1,rounds,levels,pool,identity:JSON.parse(identity.stdout),
    binary_sha256:createHash('sha256').update(fs.readFileSync(binary)).digest('hex'),
    environment:{node:process.version,os:os.platform()+' '+os.release(),arch:os.arch(),cpu:os.cpus()[0].model,
      postgres:(await owner.query('SHOW server_version')).rows[0].server_version},
    target:{concurrent_confirmations:5,p95_ms:1000,technical_errors:0},
    methodology:'Closed-loop batches of real POST confirmations after sequential GET preparation; 3 rounds per concurrency and scenario. Independent products, last-unit contention and catalog writes. Only synthetic data. Lock/transaction durations come from structured application events; pg_stat_activity is sampled and not an exact wait counter. Timing targets are a local planning assumption, not a production SLA.',runs:[]};
  const save = () => fs.writeFileSync(path.join(output,'checkout-results.json'),JSON.stringify(report,null,2));
  await owner.query(`UPDATE email_configuration SET enabled=true,from_name='Synthetic shop',from_email='shop@example.test',admin_recipients=ARRAY['admin@example.test'];
    INSERT INTO checkout_countries(country_code) VALUES('IT') ON CONFLICT DO NOTHING;
    UPDATE shipping_methods SET active=(id=2),free_from_cents=0;
    INSERT INTO shipping_method_countries(method_id,country_code) VALUES(2,'IT') ON CONFLICT DO NOTHING;
    UPDATE payment_methods SET active=(id=2),fee_kind='fixed',fee_value=0;
    INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id)
    SELECT 'Checkout '||n,'LOAD-'||n,'load-'||n,1000,10000,'published',(SELECT default_class_id FROM pricing_settings WHERE id=1)
    FROM generate_series(1,21) n;`);
  const products=(await owner.query("SELECT id FROM catalog_products WHERE model LIKE 'LOAD-%' ORDER BY id")).rows.map(x=>x.id);
  assert.equal(products.length,21);
  const columns=(await owner.query("SELECT column_name,data_type FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='checkout_details' ORDER BY ordinal_position")).rows;
  const prepare = async (product, locale) => {
    const token=randomBytes(32).toString('hex'),csrf=randomBytes(32).toString('hex');
    const cart=(await owner.query('INSERT INTO guest_carts(token_hash,csrf,revision) VALUES($1,$2,1) RETURNING id',[createHash('sha256').update(token).digest('hex'),csrf])).rows[0].id;
    await owner.query(`INSERT INTO guest_cart_items(cart_id,item_key,product_id,had_variant,quantity,name,code,unit_cents,basis_points,price_mode,currency)
      SELECT $1,'product-'||p.id,p.id,false,1,p.name,p.model,p.price_cents,2200,s.price_mode,'EUR' FROM catalog_products p CROSS JOIN pricing_settings s WHERE p.id=$2 AND s.id=1`,[cart,product]);
    const details=Object.fromEntries(columns.map(c=>[c.column_name,c.data_type==='boolean'?false:c.data_type==='bigint'?0:'']));
    Object.assign(details,{cart_id:cart,email:'buyer@example.test',first_name:'Alex',last_name:'Example',phone:'+393331234567',recipient_first_name:'Alex',recipient_last_name:'Example',shipping_country:'IT',shipping_street:'Via Example',shipping_number:'1',shipping_postal:'00100',shipping_city:'Roma',shipping_region:'RM',customer_type:'private',invoice_channel:'none',billing_same:true});
    await owner.query('INSERT INTO checkout_details SELECT (jsonb_populate_record(NULL::checkout_details,$1::jsonb)).*',[JSON.stringify(details)]);
    await owner.query('INSERT INTO checkout_methods(cart_id,shipping_method_id,payment_method_id) VALUES($1,2,2)',[cart]);
    const route=(locale==='en'?'/en':'')+'/checkout/methods',cookie='talocart_cart='+token;
    const response=await fetch(origin+route,{headers:{cookie},redirect:'manual',signal:AbortSignal.timeout(15000)});
    assert.equal(response.status,200,'Prepared checkout must render');
    return {cart,route,cookie,form:{...confirmationFields(await response.text()),shipping_method_id:'2',payment_method_id:'2',action:'confirm'}};
  };
  const send=async input=>{
    const began=performance.now();
    const response=await fetch(origin+input.route,{method:'POST',redirect:'manual',headers:{cookie:input.cookie,'content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams(input.form),signal:AbortSignal.timeout(15000)});
    await response.arrayBuffer();return {status:response.status,elapsed:performance.now()-began,location:response.headers.get('location')};
  };
  await stop();await start('warn,talocart::checkout=info');
  for(const scenario of ['independent-products','last-unit','catalog-writes']) for(const concurrency of levels) {
    const timings=[],statuses=[],events=[],waits={samples:0,blocked_samples:0,max_blocked:0,max_connections:0};let writes=0,retries=0;
    for(let round=0;round<rounds;round++) {
      await owner.query('UPDATE catalog_products SET stock=$1 WHERE id=ANY($2::bigint[])',[scenario==='last-unit'?1:10000,products]);
      const inputs=[];
      for(let worker=0;worker<concurrency;worker++)inputs.push(await prepare(products[scenario==='last-unit'?0:worker],worker%2?'en':'it'));
      clearLogs();let done=false;
      const monitor=(async()=>{while(!done){const rows=(await owner.query('SELECT cardinality(pg_blocking_pids(pid)) blockers FROM pg_stat_activity WHERE datname=current_database() AND application_name=$1',[schema])).rows;
        waits.samples++;const blocked=rows.filter(x=>x.blockers>0).length;waits.blocked_samples+=Number(blocked>0);waits.max_blocked=Math.max(waits.max_blocked,blocked);waits.max_connections=Math.max(waits.max_connections,rows.length);await delay(10);}})();
      const writer=scenario==='catalog-writes'?(async()=>{while(!done){await owner.query('UPDATE catalog_products SET description=description WHERE id=$1',[products[20]]);writes++;await delay(10);}})():Promise.resolve();
      let results;const began=performance.now();
      try {const settled=await Promise.allSettled(inputs.map(send));for(const result of settled)if(result.status==='rejected')throw result.reason;results=settled.map(x=>x.value);}
      finally{done=true;await Promise.all([monitor,writer]);}
      const elapsed=performance.now()-began;
      const successes=results.filter(x=>x.status===303).length;
      assert.equal(successes,scenario==='last-unit'?1:concurrency,'No lost successful confirmations');
      assert.equal(technicalFailures(results.map(x=>x.status),scenario==='last-unit'?[409,422]:[]),0,'No technical HTTP failures');
      const ids=inputs.map(x=>x.cart);
      const rows=(await owner.query('SELECT id FROM orders WHERE source_cart_id=ANY($1::bigint[]) AND owner_hash<>repeat(\'a\',64)',[ids])).rows;
      assert.equal(rows.length,successes,'Exactly one order per successful cart');
      const orderIDs=rows.map(x=>x.id);
      assert.equal(Number((await owner.query('SELECT count(*) n FROM order_stock_moves m JOIN order_lines l ON l.id=m.line_id WHERE l.order_id=ANY($1::bigint[])',[orderIDs])).rows[0].n),successes,'One stock movement per order');
      assert.equal(Number((await owner.query('SELECT count(*) n FROM email_outbox WHERE order_id=ANY($1::bigint[])',[orderIDs])).rows[0].n),successes*2,'Atomic customer/admin outbox');
      if(scenario==='last-unit')assert.equal((await owner.query('SELECT stock FROM catalog_products WHERE id=$1',[products[0]])).rows[0].stock,0,'No overselling');
      await delay(50);const observed=getLogs().filter(x=>x.target==='talocart::checkout'&&x.fields?.operation==='confirmation');
      assert.equal(observed.length,concurrency+getLogs().filter(x=>x.fields?.operation==='confirmation_retry').length,'Every confirmation attempt must emit safe timings');events.push(...observed.map(x=>x.fields));retries+=getLogs().filter(x=>x.fields?.operation==='confirmation_retry').length;
      const successful=results.findIndex(x=>x.status===303);const replay=await send(inputs[successful]);assert.equal(replay.status,303);assert.equal(replay.location,results[successful].location,'Replay returns committed order');
      assert.equal(Number((await owner.query('SELECT count(*) n FROM orders WHERE source_cart_id=ANY($1::bigint[]) AND owner_hash<>repeat(\'a\',64)',[ids])).rows[0].n),successes,'Replay creates no duplicate');
      timings.push(...results.map(x=>x.elapsed));statuses.push(...results.map(x=>x.status));
      events.push({batch_elapsed_ms:elapsed,batch_successes:successes});
    }
    const confirmations=events.filter(x=>x.operation==='confirmation');
    const batch=events.filter(x=>x.batch_elapsed_ms!==undefined);
    const row={scenario,concurrency,http:stats(timings),statuses:Object.fromEntries([...new Set(statuses)].map(s=>[s,statuses.filter(x=>x===s).length])),technical_errors:technicalFailures(statuses,scenario==='last-unit'?[409,422]:[]),
      confirmations_per_second:+(batch.reduce((s,x)=>s+x.batch_successes,0)*1000/batch.reduce((s,x)=>s+x.batch_elapsed_ms,0)).toFixed(2),
      pool_wait:stats(confirmations.map(x=>x.pool_wait_us/1000)),lock_wait:stats(confirmations.map(x=>x.lock_wait_us/1000)),transaction:stats(confirmations.map(x=>x.transaction_us/1000)),
      retries,catalog_writes:writes,waits};
    assert.ok(waits.max_connections<=pool);report.runs.push(row);save();console.log(JSON.stringify({scenario,concurrency,p95_ms:row.http.p95_ms,lock_p95_ms:row.lock_wait.p95_ms,technical_errors:row.technical_errors}));
  }
  report.target_met=report.runs.filter(x=>x.concurrency===report.target.concurrent_confirmations).every(x=>x.http.p95_ms<=report.target.p95_ms&&x.technical_errors===0);
  report.decision=report.target_met?'Retain the commercial lock for this measured local target; higher throughput needs a separate measured design.':'Investigate the measured dominant wait before changing lock scope.';
  save();
};
