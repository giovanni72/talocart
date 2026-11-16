// Synthetic PostgreSQL schema and private files only. Real CLI/HTTP; no provider calls.
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path'),net=require('node:net');
const {spawn,spawnSync}=require('node:child_process'),{createHash,randomBytes}=require('node:crypto');
const {Client}=require('pg');
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
(async()=>{
 const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);
 assert.ok(['127.0.0.1','localhost','[::1]'].includes(url.hostname)&&url.pathname.endsWith('_test'),'Dedicated local test DB required');
 const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/debug/talocart');
 const folder=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'talocart-verify-')));fs.chmodSync(folder,0o700);
 const schema='talocart_verify_'+process.pid+'_'+Date.now(),owner=new Client({connectionString:url.href});await owner.connect();
 const output=path.resolve('.local/verification/unified-updates/http-verify');fs.mkdirSync(output,{recursive:true,mode:0o700});
 const env=Object.fromEntries(Object.entries(process.env).filter(([k])=>!k.startsWith('TALOCART_')));
 let server,client;const config=path.join(folder,'talocart.toml');
 const run=(command,input,ok=true)=>{
  const result=spawnSync(binary,['--config',config,...command],{env,cwd:folder,input,encoding:'utf8',timeout:120000});
  fs.appendFileSync(path.join(output,'cli.log'),`Command ${command[0]}: ${result.status}\n${result.stdout||''}\n${result.stderr||''}\n`,{mode:0o600});
  if(ok)assert.equal(result.status,0,'Offline CLI failed; inspect private synthetic log');
  else assert.notEqual(result.status,0,'Invalid metadata unexpectedly accepted');
  return result.stdout.trim();
 };
 const stop=async()=>{if(!server)return;const child=server;server=null;child.kill('SIGTERM');for(let i=0;i<100;i++){if(child.exitCode!==null||child.signalCode!==null){assert.equal(child.exitCode,0,'Graceful stop');return;}await sleep(100);}child.kill('SIGKILL');throw Error('Verification shutdown timeout');};
 try{
  await owner.query(`CREATE SCHEMA ${schema}`);url.searchParams.set('options',`-csearch_path=${schema}`);client=new Client({connectionString:url.href});await client.connect();
  const port=await new Promise((resolve,reject)=>{const socket=net.createServer();socket.once('error',reject);socket.listen(0,'127.0.0.1',()=>{const p=socket.address().port;socket.close(()=>resolve(p));});});
  const origin=`http://127.0.0.1:${port}`;
  fs.writeFileSync(config,`[server]\nlisten="127.0.0.1:${port}"\n[database]\nurl=${JSON.stringify(url.href)}\n[media]\nroot=${JSON.stringify(path.join(folder,'media'))}\n[packages]\nroot=${JSON.stringify(path.join(folder,'packages'))}\n`,{mode:0o600});
  const start=async(command,expected)=>{
   const fd=fs.openSync(path.join(output,'server.log'),'a',0o600);server=spawn(binary,['--config',config,command],{env,cwd:folder,stdio:['ignore',fd,fd]});fs.closeSync(fd);
   for(let i=0;i<150;i++){assert.equal(server.exitCode,null,'Host exited; inspect private synthetic log');try{const r=await fetch(origin+'/health/ready');if(r.status===200&&await r.text()===expected)return;}catch{}await sleep(100);}throw Error('Readiness timeout');
  };
  run(['migrate']);await start('serve','ready\n');
  let r=await fetch(origin+'/setup'),html=await r.text();const cookie=r.headers.get('set-cookie').split(';')[0],csrf=html.match(/name="csrf" value="([^"]+)"/)[1];
  r=await fetch(origin+'/setup',{method:'POST',redirect:'manual',headers:{cookie,'content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams({csrf,email:'owner@example.test',password:randomBytes(20).toString('hex'),name:'Verification fixture',display_name:'Owner',primary_locale:'en',locale:'en'})});assert.equal(r.status,303);
  await stop();
  const unchecked=JSON.parse(run(['package-update-plan']));assert.equal(unchecked.database_checked,false);assert.equal(unchecked.database_sha256,null);
  const plan=JSON.parse(run(['package-update-plan','--check-database']));assert.equal(plan.database_checked,true);assert.match(plan.database_sha256,/^[a-f0-9]{64}$/);assert.equal(plan.inventory_sha256,unchecked.inventory_sha256);
  const original=JSON.parse(run(['package-update-suspend','--inventory-sha256',plan.inventory_sha256]));
  run(['package-update-migrate']);run(['package-update-apply']);run(['package-update-verify']);
  run(['package-update-restore'],JSON.stringify(original));
  run(['package-update-restore'],' '.repeat(16385),false);
  const database=async()=>{
   const tables=(await client.query("SELECT table_name FROM information_schema.tables WHERE table_schema=$1 AND table_type='BASE TABLE' ORDER BY table_name",[schema])).rows;
   const hash=createHash('sha256');
   for(const {table_name:name} of tables){assert.match(name,/^[a-z_][a-z0-9_]*$/);const rows=await client.query(`SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb) AS data FROM ${name} t`);hash.update(name);hash.update(JSON.stringify(rows.rows[0].data));}
   return hash.digest('hex');
  };
  const files=base=>{const found={};const visit=dir=>{if(!fs.existsSync(dir))return;for(const name of fs.readdirSync(dir).sort()){const full=path.join(dir,name),info=fs.lstatSync(full);assert.ok(!info.isSymbolicLink());if(info.isDirectory())visit(full);else found[path.relative(base,full)]=createHash('sha256').update(fs.readFileSync(full)).digest('hex');}};visit(base);return found;};
  fs.mkdirSync(path.join(folder,'media/library'),{recursive:true});fs.writeFileSync(path.join(folder,'media/library/probe.bin'),'synthetic original; preserve');
  const before=await database(),media=files(path.join(folder,'media')),packages=files(path.join(folder,'packages')),journal=files(path.join(folder,'storage/logs'));
  await start('serve-verify','verification-ready\n');
  for(const pathname of ['/health/live','/health/ready','/health/ready?all_modules=true','/','/catalog','/cart?preview=1','/assets/ui.css']){
   r=await fetch(origin+pathname);assert.equal(r.status,200,pathname);assert.equal(r.headers.get('x-talocart-verification'),'1',pathname);assert.ok((await r.text()).length>0);
  }
  for(const [method,pathname] of [['POST','/setup'],['POST','/admin/login'],['POST','/cart'],['GET','/cart'],['GET','/admin/login'],['GET','/admin'],['GET','/admin/extensions/fixture-module']]){r=await fetch(origin+pathname,{method,redirect:'manual'});assert.equal(r.status,503,`${method} ${pathname}`);}
  run(['package-update-verify'],undefined,false); // exclusive host lease is held by the verification host
  await sleep(1200);assert.equal(await database(),before,'Verification must not mutate database rows');
  assert.deepEqual(files(path.join(folder,'media')),media,'Media are unchanged');assert.deepEqual(files(path.join(folder,'packages')),packages,'Module files and registry are unchanged');assert.deepEqual(files(path.join(folder,'storage/logs')),journal,'Private error journal is unchanged');
  await stop();
  // Stateful smoke belongs to controlled normal activation, after the read-only
  // phase has proven data/files unchanged. The future OS gate remains closed here.
  await start('serve','ready\n');
  for(const pathname of ['/cart','/admin/login']){r=await fetch(origin+pathname);assert.equal(r.status,200);assert.equal(r.headers.get('x-talocart-verification'),null);}
  for(const locale of ['en','it']){r=await fetch(origin+'/admin/login',{headers:{cookie:'talocart_admin_language='+locale}});assert.equal(r.status,200);assert.ok((await r.text()).includes(locale==='it'?'Accedi':'Sign in'));}
  await stop();
  fs.writeFileSync(path.join(output,'result.json'),JSON.stringify({passed:true,readonly_http:true,distinct_readiness:true,mutations_blocked:true,database_unchanged:true,media_unchanged:true,packages_unchanged:true,offline_cli:true,lease_exclusive:true,stateful_http_after_activation:true,unchanged_scope:'serve-verify'},null,2));
  console.log('Read-only verification host and offline CLI passed');
 }finally{
  await stop();if(client)await client.end();await owner.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);await owner.end();fs.rmSync(folder,{recursive:true,force:true});
 }
})().catch(error=>{console.error(error.stack);process.exitCode=1;});
