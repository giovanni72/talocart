// Production SQL, dedicated migrated schema, synthetic orders only.
// Freeze only statement_timestamp() for deterministic calendar tests; predicates stay intact.
const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), os = require('node:os');
const { spawnSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { performance } = require('node:perf_hooks');
const { Client } = require('pg');

async function main() {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test database required');
  const benchmark = process.argv.includes('--benchmark');
  assert.ok(process.argv.slice(2).every(a => a === '--benchmark'), 'Unknown argument');
  const readSql = file => {
    const matches = [...fs.readFileSync(file, 'utf8').matchAll(/sqlx::query_as\(\s*("(?:[^"\\]|\\[\s\S])*")\s*\)/g)];
    assert.equal(matches.length, 1, 'Exactly one production sales-history query');
    return JSON.parse(matches[0][1].replace(/\\\r?\n\s*/g, ''));
  };
  const sql = readSql('crates/commerce/src/overview/sales.rs');
  assert.equal(sql.match(/statement_timestamp\(\)/g)?.length, 2, 'Clock substitution must stay explicit');
  const frozen = sql.replaceAll('statement_timestamp()', '$4::timestamptz');
  const baseline = benchmark && process.env.TALOCART_TEST_SALES_BASELINE_SOURCE ? readSql(process.env.TALOCART_TEST_SALES_BASELINE_SOURCE) : null;
  if (baseline) assert.equal(baseline.match(/statement_timestamp\(\)/g)?.length, 2);
  const previous = baseline?.replaceAll('statement_timestamp()', '$4::timestamptz');
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/debug/talocart');
  const output = path.resolve(process.env.TALOCART_TEST_SALES_OUTPUT || path.join('.local/verification/sales-history', benchmark ? 'benchmark' : 'dst'));
  fs.mkdirSync(output, { recursive: true });
  const report = { sql_sha256: createHash('sha256').update(sql).digest('hex'), binary_sha256: createHash('sha256').update(fs.readFileSync(binary)).digest('hex'), host: { platform: os.platform(), arch: os.arch(), cpu: os.cpus()[0]?.model, memory_gib: os.totalmem()/1024**3 }, clock: 'SQL statement clock replaced by a bound instant only', cases: [] };
  if (baseline) report.baseline_sql_sha256 = createHash('sha256').update(baseline).digest('hex');
  const db = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL });
  await db.connect();
  const schema = 'sales_history_' + process.pid + '_' + Date.now();
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'sales-history-'));
  let created = false;
  try {
    await db.query('CREATE SCHEMA ' + schema); created = true;
    await db.query('SET search_path TO ' + schema);
    await db.query("SET statement_timeout='120s'");
    const url = new URL(process.env.TALOCART_TEST_DATABASE_URL); url.searchParams.set('options', '-csearch_path=' + schema);
    const config = path.join(folder, 'talocart.toml'); fs.writeFileSync(config, '', { mode: 0o600 });
    const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith('TALOCART_')));
    Object.assign(env, { TALOCART_DATABASE_URL: url.href, TALOCART_MEDIA_ROOT: path.join(folder, 'media') });
    const migration = spawnSync(binary, ['--config', config, 'migrate'], { env, encoding: 'utf8', timeout: 120000 });
    assert.equal(migration.status, 0, 'Dedicated schema migration failed');
    report.postgres = (await db.query('SHOW server_version')).rows[0].server_version;
    await db.query("UPDATE platform_regional_settings SET time_zone='Europe/Rome'");
    const statuses = (await db.query('SELECT id FROM order_statuses ORDER BY id LIMIT 2')).rows.map(r=>Number(r.id));
    assert.equal(statuses.length, 2);
    const insert = `INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents,created_at)
      VALUES($1,$2,0,repeat('a',64),$3,'EUR',$4,0,0,$4,0,$4,$5)`;
    if (!benchmark) {
      for (const [day, hours, repeated] of [['2026-03-29',23,['2026-03-29T00:30:00Z','2026-03-29T01:30:00Z']],['2026-10-25',25,['2026-10-25T00:30:00Z','2026-10-25T01:30:00Z']]]) {
        const bounds = (await db.query("SELECT $1::date::timestamp AT TIME ZONE 'Europe/Rome' AS start,($1::date+1)::timestamp AT TIME ZONE 'Europe/Rome' AS finish", [day])).rows[0];
        assert.equal((bounds.finish-bounds.start)/3600000, hours);
        const start=bounds.start.toISOString(), end=bounds.finish.toISOString();
        const fixtures = [[new Date(bounds.start-1),900,statuses[0]], [start,101,statuses[0]], [repeated[0],202,statuses[0]], [repeated[1],303,statuses[0]], [new Date(bounds.finish-1),404,statuses[0]], [end,800,statuses[0]], [repeated[1],700,statuses[1]]];
        await db.query('BEGIN');
        try {
          let n=0; for (const [instant,cents,status] of fixtures) { n++; await db.query(insert,[day+'-'+n,n,'status_id = '+status,cents,instant]); }
          for (const sessionZone of ['UTC','America/New_York']) {
            await db.query("SELECT set_config('TimeZone',$1,false)",[sessionZone]);
            for (const [monthly,buckets] of [[false,7],[true,12]]) {
              const rows=(await db.query(frozen,[monthly,buckets,[statuses[0]],end])).rows;
              assert.equal(rows.length,buckets);
              const target=rows.find(r=>r.date===(monthly?day.slice(0,7):day));
              // Monthly includes the preceding/following day; daily separates both midnights.
              assert.equal(Number(target.orders),monthly?6:4);
              assert.equal(Number(target.cents),monthly?2710:1010);
              if (!monthly) {
                const nextDay=new Date(Date.parse(day+'T12:00:00Z')+86400000).toISOString().slice(0,10);
                assert.equal(Number(rows.find(r=>r.date===nextDay)?.orders),1, 'Next midnight belongs to the following day');
              }
              report.cases.push({day,hours,session_zone:sessionZone,monthly,passed:true});
            }
            const boundaryClock=new Date(Date.parse(day+'T12:00:00Z')+6*86400000).toISOString();
            const boundary=(await db.query(frozen,[false,7,[statuses[0]],boundaryClock])).rows;
            assert.equal(boundary[0].date,day,'Seven calendar days start at local midnight, across DST');
            assert.equal(boundary.reduce((n,r)=>n+Number(r.orders),0),5);
            assert.equal(boundary.reduce((n,r)=>n+Number(r.cents),0),1810);
            const localHours=(await db.query("SELECT to_char(t AT TIME ZONE 'Europe/Rome','HH24:MI') AS hour FROM unnest($1::timestamptz[]) t",[repeated])).rows.map(r=>r.hour);
            assert.deepEqual(localHours,hours===25?['02:30','02:30']:['01:30','03:30']);
            const empty=(await db.query(frozen,[false,7,[],end])).rows;
            assert.ok(empty.every(r=>r.orders==='0'&&r.cents==='0'));
          }
        } finally { await db.query('ROLLBACK'); }
      }
    } else {
      const asof='2026-11-01T12:00:00Z', samples=7, budget=2000;
      report.reference=asof; report.samples=samples; report.p95_budget_ms=budget;
      for (const size of [100000,1000000]) {
        // Each size starts from an empty schema's order tables, retaining real constraints/triggers.
        await db.query('TRUNCATE orders,order_current_status,order_status_events,order_lines,order_customer_summaries CASCADE');
        await db.query(`INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents,created_at)
          SELECT 'BENCH-'||n,n,0,repeat('a',64),'status_id = '||CASE WHEN n%2=0 THEN $2::bigint ELSE $3::bigint END,CASE WHEN n%3=0 THEN 'USD' ELSE 'EUR' END,101+n%1000,0,0,101+n%1000,0,101+n%1000,$4::timestamptz-(n%730)*interval '1 day' FROM generate_series(1,$1::int) n`,[size,...statuses,asof]);
        for(const table of ['orders','order_current_status','order_status_events']) await db.query('VACUUM (ANALYZE) '+table);
        for(const [period,monthly,buckets] of [['7d',false,7],['30d',false,30],['90d',false,90],['12m',true,12]]) for(const selected of [statuses,[statuses[0]],[]]) {
          const expected=new Map();
          for(let n=1;n<=size;n++) {
            const age=n%730, date=new Date(Date.parse(asof)-age*86400000).toISOString().slice(0,10);
            if(!selected.includes(statuses[n%2===0?0:1]) || (monthly?date<'2025-12-01':age>=buckets)) continue;
            const key=(n%3===0?'USD':'EUR')+'|'+(monthly?date.slice(0,7):date), value=expected.get(key)||[0,0]; value[0]++;value[1]+=101+n%1000;expected.set(key,value);
          }
          const parameters=[monthly,buckets,selected,asof];
          const verify=rows=>{const actual=new Map(rows.filter(r=>Number(r.orders)>0).map(r=>[r.currency+'|'+r.date,[Number(r.orders),Number(r.cents)]]));assert.deepEqual(actual,expected);assert.equal(rows.length,buckets*(expected.size?2:1));};
          const warmed=(await db.query(frozen,parameters)).rows;verify(warmed); // not measured
          if (previous) {
            const old=(await db.query(previous,parameters)).rows;verify(old);
            assert.deepEqual(warmed,old,'Both queries preserve every zero-filled bucket');
          }
          const times=[], before=[];
          for(let i=0;i<samples;i++) {
            // Alternate the order on the same fixture, connection and frozen instant.
            for(const [query,target] of (i%2 ? [[frozen,times],[previous,before]] : [[previous,before],[frozen,times]])) {
              if (!query) continue;
              const start=performance.now(); const result=await db.query(query,parameters);target.push(performance.now()-start);verify(result.rows);
            }
          }
          times.sort((a,b)=>a-b);const p95=times[Math.ceil(.95*samples)-1];
          const plan=(await db.query('EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) '+frozen,parameters)).rows[0]['QUERY PLAN'][0];
          const comparison = previous ? { before: { p50_ms:before.sort((a,b)=>a-b)[Math.floor(samples/2)], p95_ms:before[Math.ceil(.95*samples)-1], plan:(await db.query('EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) '+previous,parameters)).rows[0]['QUERY PLAN'][0] } } : {};
          report.cases.push({size,period,statuses:selected.length,p50_ms:times[Math.floor(samples/2)],p95_ms:p95,plan,...comparison,passed:p95<=budget});
          console.log(size,period,'statuses='+selected.length,'p95='+p95.toFixed(1)+'ms');
        }
      }
      assert.ok(report.cases.every(c=>c.passed),'Sales SQL p95 exceeds the declared local budget');
    }
    report.passed=true;
  } finally {
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(report,null,2));
    try { if(created) await db.query('DROP SCHEMA '+schema+' CASCADE'); } finally { await db.end(); fs.rmSync(folder,{recursive:true,force:true}); }
  }
  console.log(benchmark?'Sales benchmark passed':'Sales DST tests passed', report.cases.length, 'cases');
}
main().catch(error=>{console.error(error instanceof assert.AssertionError?error.message:'Sales verification failed; inspect local environment and fixture');process.exitCode=1;});
