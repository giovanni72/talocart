// Real outage/recovery through private loopback proxies; shared services stay running.
const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), os = require('node:os'), net = require('node:net');
const { spawn, spawnSync } = require('node:child_process');
const { randomBytes, createHash } = require('node:crypto');
const { Client } = require('pg');
const delay = ms => new Promise(resolve => setTimeout(resolve,ms));
async function proxy(port,host='127.0.0.1') {
  const sockets=new Set();let enabled=true;
  const server=net.createServer(socket=>{
    sockets.add(socket);socket.on('close',()=>sockets.delete(socket));socket.on('error',()=>{});
    if(!enabled){socket.destroy();return;}
    const upstream=net.connect(port,host);sockets.add(upstream);
    upstream.on('close',()=>sockets.delete(upstream));upstream.on('error',()=>socket.destroy());
    socket.on('close',()=>upstream.destroy());upstream.on('close',()=>socket.destroy());
    socket.pipe(upstream);upstream.pipe(socket);
  });
  await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',resolve);});
  return {port:server.address().port,set(value){enabled=value;if(!value)for(const socket of sockets)socket.destroy();},
    async close(){for(const socket of sockets)socket.destroy();await new Promise(resolve=>server.close(resolve));}};
}
(async()=>{
  const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);
  assert.ok(['localhost','127.0.0.1','[::1]'].includes(url.hostname)&&url.pathname.endsWith('_test')&&!url.search,'Dedicated local _test DB required');
  const binary=path.resolve(process.env.TALOCART_BASE_TEST_BINARY||process.env.TALOCART_TEST_BINARY||'target/debug/talocart');
  const previous=process.env.TALOCART_TEST_PREVIOUS_BINARY?path.resolve(process.env.TALOCART_TEST_PREVIOUS_BINARY):null;
  const identity=spawnSync(binary,['build-info'],{encoding:'utf8',timeout:30000});assert.equal(identity.status,0);
  const folder=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'talocart-runtime-')));fs.chmodSync(folder,0o700);
  const output=path.resolve('.local/verification/consolidation/runtime');fs.mkdirSync(output,{recursive:true});
  const schema=`talocart_runtime_${process.pid}_${Date.now()}`,tag=randomBytes(8).toString('hex');
  const owner=new Client({connectionString:url.href});let dbProxy,smtpProxy,server,created=false;
  const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('TALOCART_')));
  env.TALOCART_LOG='warn,talocart::workers=info';
  const config=path.join(folder,'talocart.toml'),logPath=path.join(folder,'server.log');
  const report={identity:JSON.parse(identity.stdout),binary_sha256:createHash('sha256').update(fs.readFileSync(binary)).digest('hex'),isolated_schema:true,shared_services_stopped:false};
  if(previous){const old=spawnSync(previous,['build-info'],{encoding:'utf8',timeout:30000});assert.equal(old.status,0);report.previous_identity=JSON.parse(old.stdout);report.previous_sha256=createHash('sha256').update(fs.readFileSync(previous)).digest('hex');}
  const wait=async(check,label)=>{for(let i=0;i<150;i++){if(await check())return;await delay(100);}throw new Error(label);};
  const status=async(route)=>{try{return (await fetch(origin+route,{signal:AbortSignal.timeout(5000)})).status;}catch{return 0;}};
  const stop=async()=>{if(!server)return;const child=server;server=undefined;child.kill('SIGTERM');try{await wait(()=>child.exitCode!==null||child.signalCode!==null,'Graceful shutdown timeout');}catch(error){child.kill('SIGKILL');await wait(()=>child.exitCode!==null||child.signalCode!==null,'Synthetic process cleanup timeout');throw error;}assert.equal(child.exitCode,0);};
  let origin;
  try {
    await owner.connect();await owner.query(`CREATE SCHEMA ${schema}`);created=true;await owner.query(`SET search_path TO ${schema}`);
    dbProxy=await proxy(Number(url.port||5432),url.hostname);smtpProxy=await proxy(Number(process.env.TALOCART_TEST_SMTP_PORT||1025));
    const portServer=net.createServer();await new Promise(resolve=>portServer.listen(0,'127.0.0.1',resolve));const port=portServer.address().port;await new Promise(resolve=>portServer.close(resolve));origin=`http://127.0.0.1:${port}`;
    const privateURL=new URL(url);privateURL.hostname='127.0.0.1';privateURL.port=String(dbProxy.port);privateURL.searchParams.set('options',`-csearch_path=${schema}`);
    const baseConfig=`[server]\nlisten="127.0.0.1:${port}"\n[database]\nurl=${JSON.stringify(privateURL.href)}\ntimeout_seconds=1\n[media]\nroot=${JSON.stringify(path.join(folder,'media'))}\n[packages]\nroot=${JSON.stringify(path.join(folder,'packages'))}\n`;
    fs.writeFileSync(config,baseConfig,{mode:0o600});
    const cli=async(command,selected=binary)=>{const child=spawn(selected,['--config',config,command],{cwd:folder,env,stdio:'ignore'});await new Promise((resolve,reject)=>{const timer=setTimeout(()=>{child.kill('SIGTERM');reject(Error('Isolated CLI timeout'));},60000);child.on('error',error=>{clearTimeout(timer);reject(error);});child.on('exit',code=>{clearTimeout(timer);code===0?resolve():reject(Error('Isolated CLI failed'));});});};
    const start=async(selected=binary)=>{const log=fs.openSync(logPath,'a',0o600);server=spawn(selected,['--config',config,'serve'],{cwd:folder,env,stdio:['ignore',log,log]});fs.closeSync(log);await wait(async()=>{assert.equal(server.exitCode,null,'Synthetic host exited');return await status('/health/ready')===200;},'Readiness timeout');};
    await cli('migrate',previous||binary);
    // Inspect the actual migrated schema, not a guessed version cutoff.
    const legacyTransport=(await owner.query("SELECT to_regclass('email_transport') IS NULL AS legacy")).rows[0].legacy;
    report.previous_transport_source=legacyTransport?'file':'database';
    if(legacyTransport)fs.writeFileSync(config,baseConfig+`[mail]\nenabled=true\nhost="127.0.0.1"\nport=${smtpProxy.port}\nsecurity="local"\ntimeout_seconds=2\n`,{mode:0o600});
    if(!previous){
      await cli('doctor');
      assert.equal(fs.existsSync(path.join(folder,'media.secrets')),false,'Doctor must not generate SMTP keys');
    }
    await start(previous||binary);
    if(!legacyTransport)assert.equal(fs.existsSync(path.join(folder,'media.secrets/smtp.key')),true,'Serve prepares its SMTP key');
    const get=async(route,cookie)=>{const response=await fetch(origin+route,{headers:cookie?{cookie}:{},redirect:'manual',signal:AbortSignal.timeout(5000)});return {response,html:await response.text()};};
    const csrf=html=>{const value=html.match(/name="csrf" value="([^"]+)"/)?.[1];assert.ok(value);return value;};
    const cookie=response=>response.headers.getSetCookie().find(v=>v.startsWith('talocart_session='))?.split(';')[0];
    const post=(route,session,fields)=>fetch(origin+route,{method:'POST',redirect:'manual',headers:{cookie:session||'','content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams(fields),signal:AbortSignal.timeout(10000)});
    let page=await get('/setup');const password=randomBytes(24).toString('hex');
    let response=await post('/setup',cookie(page.response),{csrf:csrf(page.html),email:'owner@example.test',password,name:'Runtime test',display_name:'Owner',primary_locale:'en',locale:'en'});assert.equal(response.status,303);
    page=await get('/admin/login');response=await post('/admin/login',cookie(page.response),{csrf:csrf(page.html),email:'owner@example.test',password});assert.equal(response.status,303);const session=cookie(response);assert.ok(session);
    const configureTransport=async()=>{
      const settings=await get('/admin/settings/email',session);
      const revision=(await owner.query('SELECT revision FROM email_transport WHERE id=1')).rows[0].revision;
      const saved=await post('/admin/settings/email/transport',session,{csrf:csrf(settings.html),revision:String(revision),enabled:'true',host:'127.0.0.1',port:String(smtpProxy.port),security:'local',username:'',password:'',timeout_seconds:'2'});
      assert.equal(saved.status,303,'SMTP configuration through the current panel');
    };
    if(!legacyTransport)await configureTransport();
    await owner.query("UPDATE email_configuration SET enabled=true,from_name='Runtime test',from_email='sender@example.test',admin_recipients=ARRAY['admin@example.test']");
    smtpProxy.set(false);
    page=await get('/admin/settings/email',session);
    const recipient=`runtime-${tag}@example.test`;
    response=await post('/admin/settings/email/test',session,{csrf:csrf(page.html),key:randomBytes(32).toString('hex'),recipient,template:'order_received',locale:'en'});assert.equal(response.status,303);
    await wait(async()=>Number((await owner.query("SELECT count(*) n FROM email_outbox WHERE recipient=$1 AND attempts>=1 AND state='pending' AND last_outcome IN ('transient','timeout')",[recipient])).rows[0].n)===1,'SMTP failure did not retain retryable work');
    report.smtp_outage_retained_work=true;
    await owner.query('UPDATE email_configuration SET enabled=false');
    const snapshot=async()=>(await owner.query('SELECT event_key,message_id,encode(content_hash,\'hex\') AS hash,state,attempts FROM email_outbox WHERE recipient=$1',[recipient])).rows;
    const before=await snapshot();await stop();
    // Explicit transition: remove the obsolete file block before invoking the new CLI.
    fs.writeFileSync(config,baseConfig,{mode:0o600});
    await cli('migrate');await cli('doctor');await start();
    assert.deepEqual(await snapshot(),before,'Restart preserves committed work');
    if(legacyTransport){
      assert.equal((await owner.query('SELECT enabled FROM email_transport WHERE id=1')).rows[0].enabled,false,'Upgrade must not import or enable legacy SMTP');
      await configureTransport();
      assert.deepEqual(await snapshot(),before,'Configuring SMTP must preserve the paused queue');
      report.legacy_smtp_transition_verified=true;
    }
    if(previous)report.upgrade_from_previous_preserved_queue=true;
    report.graceful_restart_preserved_queue=true;
    dbProxy.set(false);assert.equal(await status('/health/live'),200,'Liveness remains independent');assert.equal(await status('/health/ready'),503,'Readiness detects actual DB unavailability');
    report.database_outage_detected=true;const recoveryStarted=performance.now();dbProxy.set(true);await wait(async()=>await status('/health/ready')===200,'Pool did not recover');
    assert.equal((await get('/admin/settings/email',session)).response.status,200,'Session persisted through reconnect');assert.deepEqual(await snapshot(),before);
    report.database_recovery_preserved_session_and_work=true;report.database_recovery_ms=+(performance.now()-recoveryStarted).toFixed(2);
    smtpProxy.set(true);await owner.query("UPDATE email_configuration SET enabled=true; UPDATE email_outbox SET available_at=clock_timestamp() WHERE state='pending'");
    await wait(async()=>(await snapshot())[0]?.state==='accepted','SMTP did not resume persisted work');
    const api=process.env.TALOCART_TEST_MAILPIT_URL||'http://127.0.0.1:8025';
    const captured=await(await fetch(api+'/api/v1/search?query='+encodeURIComponent('to:'+recipient),{signal:AbortSignal.timeout(5000)})).json();assert.equal(captured.messages.length,1,'Exactly one captured SMTP acceptance');
    report.smtp_recovery_accepted_persisted_message=true;
    await stop();report.graceful_shutdown=true;
    const samples=fs.readFileSync(logPath,'utf8').split('\n').flatMap(line=>{try{const event=JSON.parse(line);return event.target==='talocart::workers'&&event.fields?.queue?[event.fields]:[];}catch{return [];}});
    assert.ok(samples.some(x=>x.queue==='email')&&samples.some(x=>x.queue==='payments'),'Operational queue samples required');
    const allowed=new Set(['message','queue','pending','running','failed','retrying','expired_leases','oldest_due_seconds','pool_size','pool_idle']);
    assert.ok(samples.every(row=>Object.keys(row).every(key=>allowed.has(key))),'Diagnostics contain only allowlisted fields');
    report.queue_diagnostics_safe=true;if(previous)assert.equal(createHash('sha256').update(fs.readFileSync(previous)).digest('hex'),report.previous_sha256,'Previous executable unchanged');report.passed=true;
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report));
  } finally {
    try { await stop(); } finally {
      if(dbProxy)await dbProxy.close();if(smtpProxy)await smtpProxy.close();
      try { if(created)await owner.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`); } finally { await owner.end();fs.rmSync(folder,{recursive:true,force:true}); }
    }
  }
})().catch(error=>{console.error('Runtime recovery failed:',error.code||error.message?.replace(/postgres[^\s]*/g,'[private]'));process.exitCode=1;});
