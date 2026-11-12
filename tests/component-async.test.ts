import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { QueryGeneration } from '../src/db-model.ts';
import {credentialTargetUnchanged} from '../src/profiles-model.ts';
import {browserScopeKey} from '../src/kube-browser.ts';
import {acceptKubeResult} from '../src/kube-model.ts';

// Execute the actual component handler with controlled transport completion order.
function handler(file:string,name:string,bindings:Record<string,unknown>) {
 const source=ts.createSourceFile(file,readFileSync(new URL(file,import.meta.url),'utf8'),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
 let expression:ts.Node|undefined;
 const visit=(node:ts.Node)=>{if(name==='$startupEffect'&&!expression&&ts.isCallExpression(node)&&node.expression.getText(source)==='useEffect')expression=node.arguments[0];if(ts.isVariableDeclaration(node)&&node.name.getText(source)===name)expression=node.initializer;if(ts.isFunctionDeclaration(node)&&node.name?.text===name)expression=node;ts.forEachChild(node,visit);};visit(source);
 assert.ok(expression);
 const js=ts.transpileModule(`const ${name} = ${expression.getText(source)};`,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.None,jsx:ts.JsxEmit.React}}).outputText;
 return new Function(...Object.keys(bindings),js+`\nreturn ${name};`)(...Object.values(bindings));
}
test('table inspection ignores older selections, disconnects and newer SQL edits',async()=>{
 const pending=new Map<string,(v:unknown)=>void>();
 let result:unknown;let sql='initial';
 const sessionRef={current:'session-1'};const inspectionGeneration={current:new QueryGeneration()};const sqlRevision={current:0};
 const inspect=handler('../src/DbPanel.tsx','inspect',{sessionRef,inspectionGeneration,sqlRevision,invoke:(_command:string,args:{table:string})=>new Promise(resolve=>pending.set(args.table,resolve)),setInspection:(v:unknown)=>result=v,setSql:(v:string)=>sql=v,setError:()=>{},setInspecting:()=>{}});
 const a=inspect('["demo","a"]');const b=inspect('["demo","b"]');pending.get('b')!('b');await b;pending.get('a')!('a');await a;
 assert.equal(result,'b');assert.equal(sql,'SELECT * FROM `demo`.`b`');
 const c=inspect('["demo","c"]');sql='SELECT typed_by_user';sqlRevision.current++;pending.get('c')!('c');await c;assert.equal(sql,'SELECT typed_by_user');
 const d=inspect('["demo","d"]');sessionRef.current='';inspectionGeneration.current.invalidate();pending.get('d')!('d');await d;assert.equal(result,'c');assert.equal(sql,'SELECT typed_by_user');
});
test('cancel while query creation is pending cancels the eventual backend query',async()=>{
 let complete!:(id:string)=>void;const cancelled:string[]=[];let page:unknown;
 const bindings={sessionRef:{current:'session'},queryRef:{current:undefined as string|undefined},generation:{current:new QueryGeneration()},mounted:{current:true},cancelRequested:{current:false},sql:'SELECT 999',maxRows:100,timeout:30,setError:()=>{},setRunning:()=>{},setPage:(value:unknown)=>page=value,
  invoke:async(command:string,args:Record<string,unknown>)=>{
   if(command==='db_query')return new Promise<string>(resolve=>complete=resolve);
   if(command==='db_cancel'){cancelled.push(String(args.queryId));return;}
   if(command==='db_query_page')return {status:cancelled.includes(String(args.queryId))?'cancelled':'running'};
   throw new Error('unexpected operation '+command);
  }};
 const run=handler('../src/DbPanel.tsx','run',bindings);const cancel=handler('../src/DbPanel.tsx','cancel',bindings);
 const pending=run(false);await cancel();complete('late-query');await pending;
 assert.deepEqual(cancelled,['late-query']);assert.deepEqual(page,{status:'cancelled'});
});


function connectionDialog(overrides:Record<string,unknown>={}){
 let password='synthetic-only';let error='';let saved:unknown;let requests=0;
 const value={name:'fixture',host:'127.0.0.1',port:3306,username:'reader',database:'demo',tls:'disabled',password};
 const form={validateFields:async()=>value,setFieldValue:(key:string,v:string)=>{if(key==='password')password=v;}};
 const bindings={form,saving:{current:false},mounted:{current:true},profile:null,mode:'replace',settings:(v:typeof value)=>({driver:'mysql',host:v.host,port:v.port,username:v.username,database:v.database,tls:v.tls}),setBusy:()=>{},setError:(v:string)=>error=v,onSaved:(v:unknown)=>saved=v,invoke:async(command:string,args:{credential:{value:string}})=>{requests++;assert.equal(command,'profiles_save');assert.equal(args.credential.value,'synthetic-only');return {profile:{id:'saved'},durabilityConfirmed:true,cleanupPending:0};},...overrides};
 return {bindings,value,snapshot:()=>({password,error,saved,requests})};
}
test('profile modal saves through backend and clears input only after a committed receipt',async()=>{
 const d=connectionDialog();await handler('../src/ConnectionDialog.tsx','save',d.bindings)();assert.equal(d.snapshot().requests,1);assert.equal(d.snapshot().password,'');assert.ok(d.snapshot().saved);
});
test('profile modal retains input after credential service failure for retry',async()=>{
 const d=connectionDialog({invoke:async()=>{throw 'system credential service unavailable';}});await handler('../src/ConnectionDialog.tsx','save',d.bindings)();assert.equal(d.snapshot().saved,undefined);assert.equal(d.snapshot().password,'synthetic-only');assert.equal(d.snapshot().error,'system credential service unavailable');
});
test('profile save prevents duplicate requests before validation resolves',async()=>{
 let complete!:(v:unknown)=>void;const d=connectionDialog({form:{validateFields:()=>new Promise(resolve=>complete=resolve),setFieldValue:()=>{}}});const save=handler('../src/ConnectionDialog.tsx','save',d.bindings);const a=save();const b=save();complete(d.value);await Promise.all([a,b]);assert.equal(d.snapshot().requests,1);
});
test('profile modal cancel clears password and leaves a pending save alone',()=>{
 let closed=0;const d=connectionDialog({onClose:()=>closed++});const cancel=handler('../src/ConnectionDialog.tsx','cancel',d.bindings);(d.bindings.saving as {current:boolean}).current=true;cancel();assert.equal(closed,0);(d.bindings.saving as {current:boolean}).current=false;cancel();assert.equal(closed,1);assert.equal(d.snapshot().password,'');
});
test('selecting a running profile focuses its stable workspace without another connection',async()=>{
 let selected='';let calls=0;
 const open=handler('../src/ConnectionManager.tsx','openProfile',{currentWorkspaces:{current:[{id:'runtime-A',profileId:'profile-A'}]},setActive:(v:string)=>selected=v,invoke:async()=>calls++});
 await open({id:'profile-A',revision:99});assert.equal(selected,'runtime-A');assert.equal(calls,0);
});
test('late log start cannot install into or stop another runtime with the same generation',async()=>{
 const calls:Record<string,unknown>[]=[];let installed=false;
 const bindings={selectedSnapshot:{current:{scope:{sessionId:'A',generation:1,resourceId:'pods'},data:{catalogRevision:0,scopeRevision:1}}},controller:{state:{interactive:true}},identityArgs:handler('../src/KubePanel.tsx','identityArgs',{}),logRef:{current:null},selection:{sessionId:'A',generation:1},selected:{name:'pod'},container:'app',follow:true,logEpoch:{current:0},visibleLog:{current:''},setLogs:()=>{},setDropped:()=>{},setLogStatus:()=>{},setError:()=>{},setStartingLogs:()=>{},setLogId:()=>installed=true,session:{current:{sessionId:'B',generation:1}},acceptKubeResult:(a:{sessionId:string;generation:number},b:{sessionId:string;generation:number})=>a.sessionId===b.sessionId&&a.generation===b.generation,
 invoke:async(_command:string,args:Record<string,unknown>)=>{calls.push(args);return {sessionId:'A',generation:1,logId:'log-A'};}};
 await handler('../src/KubePanel.tsx','startLogs',bindings)();assert.equal(installed,false);assert.equal(calls.length,2);assert.ok(calls.every(args=>args.sessionId==='A'));assert.equal(calls[1].logId,'log-A');
});

test('late context switch response cannot restore a workspace that was closed meanwhile',async()=>{
 let complete!:(value:unknown)=>void;let applied=0;let calls=0;const session={current:{sessionId:'A',generation:1} as {sessionId:string;generation:number}|null};
 const bindings={session,transitioning:{current:false},mounted:{current:true},controller:{invalidate:()=>{}},clearDetails:()=>{},setSwitching:()=>{},setError:()=>{},setSelection:()=>applied++,invoke:()=>{calls++;return new Promise(resolve=>complete=resolve);}};
 const switchSelection=handler('../src/KubePanel.tsx','switchSelection',bindings);const pending=switchSelection('other');await switchSelection('third');assert.equal(calls,1);session.current=null;complete({sessionId:'A',generation:2});await pending;assert.equal(applied,0);
});

test('committed delete receipt states deletion and leaves cleanup status explicit',()=>{
 let notice='';const saved=handler('../src/ConnectionManager.tsx','saved',{setDialog:()=>{},setError:()=>{},setNotice:(value:string)=>notice=value,refresh:async()=>{}});
 saved({profile:null,durabilityConfirmed:true,cleanupPending:1});assert.match(notice,/已删除/);assert.match(notice,/清理/);
 saved({profile:null,durabilityConfirmed:false,cleanupPending:1});assert.match(notice,/已删除/);assert.match(notice,/持久性尚未确认/);
});


test('legacy migration rejection cannot hide an existing backend list on startup or restart',async()=>{
 const storage={getItem:()=>null};let lists=0;let visible:unknown;let notice='';
 const bindings={mounted:{current:true},localStorage:storage,legacyProfiles:()=>({profiles:[{sourceId:'reject'}],invalidCount:1}),invoke:async()=>{throw 'metadata I/O rejected';},refresh:async()=>{lists++;visible=['existing-profile'];},setError:()=>{},setNotice:(value:string)=>notice=value};
 const effect=handler('../src/ConnectionManager.tsx','$startupEffect',bindings);
 effect();await new Promise(resolve=>setImmediate(resolve));assert.ok(lists>0);assert.deepEqual(visible,['existing-profile']);assert.match(notice,/未迁移|未完成|保留/);
 effect();await new Promise(resolve=>setImmediate(resolve));assert.ok(lists>=2);assert.deepEqual(visible,['existing-profile']);
});
test('a rejected legacy entry cannot stop valid entries migrating and old storage is preserved',async()=>{
 const raw='unchanged old metadata';let imports:string[]=[];let lists=0;let notice='';
 const bindings={mounted:{current:true},localStorage:{getItem:()=>raw},legacyProfiles:()=>({profiles:[{sourceId:'bad'},{sourceId:'good'}],invalidCount:0}),invoke:async(_command:string,args:{profiles:{sourceId:string}[]})=>{assert.equal(args.profiles.length,1);const id=args.profiles[0].sourceId;imports.push(id);if(id==='bad')throw 'rejected';return 1;},refresh:async()=>lists++,setError:()=>{},setNotice:(value:string)=>notice=value};
 handler('../src/ConnectionManager.tsx','$startupEffect',bindings)();await new Promise(resolve=>setImmediate(resolve));assert.deepEqual(imports,['bad','good']);assert.ok(lists>0);assert.match(notice,/未迁移|保留/);assert.equal(bindings.localStorage.getItem(),raw);
});
test('clearing a saved default kube context is serialized as null',async()=>{
 let requested:unknown;
 const original={snapshotId:'snapshot',contexts:['a','b'],context:'a',namespace:null};
 const bindings={working:{current:false},setBusy:()=>{},setError:()=>{},form:{validateFields:async()=>({name:'cluster',context:undefined,namespace:''})},replace:false,original,contextInitialized:{current:true},profile:{id:'profile',revision:1},mounted:{current:true},binding:{current:{}},setText:()=>{},onSaved:()=>{},invoke:async(_command:string,args:{draft:{settings:unknown}})=>{requested=args.draft.settings;return {profile:{},durabilityConfirmed:true,cleanupPending:0};}};
 await handler('../src/KubeImportDialog.tsx','save',bindings)();assert.equal((requested as {context:unknown}).context,null);
});

test('file and name can save with the imported default before context controls initialize',async()=>{
 let requested:unknown;const settings={snapshotId:'snapshot',contexts:['a','b'],context:'a',namespace:'ops'};
 const bindings={working:{current:false},setBusy:()=>{},setError:()=>{},form:{validateFields:async()=>({name:'cluster'})},replace:true,original:null,contextInitialized:{current:false},profile:null,mounted:{current:true},binding:{current:{}},setText:()=>{},onSaved:()=>{},parseSource:async()=>({token:'import-token',settings}),invoke:async(_command:string,args:{draft:{settings:unknown}})=>{requested=args.draft.settings;return {profile:{},durabilityConfirmed:true,cleanupPending:0};}};
 await handler('../src/KubeImportDialog.tsx','save',bindings)();assert.equal((requested as {context:unknown}).context,'a');
});

test('new database test validates connection fields without name and never saves or opens a workspace',async()=>{
 let validated:string[]=[];const calls:string[]=[];let result:unknown;
 const bindings={saving:{current:false},testRevision:{current:0},mounted:{current:true},profile:null,driver:'doris',form:{validateFields:async(fields:string[])=>{validated=fields;return {host:'db.test',port:9030,username:'reader',password:'synthetic-only',database:'',tls:'disabled'};}},setBusy:()=>{},setTesting:()=>{},setTestResult:(v:unknown)=>result=v,invoke:async(command:string,args:{config:Record<string,unknown>})=>{calls.push(command);assert.equal('driver'in args.config,false);assert.equal(args.config.kind,'doris');},settings:()=>({driver:'doris',host:'db.test',port:9030,username:'reader',database:'',tls:'disabled'})};
 await handler('../src/ConnectionDialog.tsx','testConnection',bindings)();assert.ok(!validated.includes('name'));assert.deepEqual(calls,['db_test_connection']);assert.equal((result as {kind:string}).kind,'success');
});
test('duplicate test is blocked before validation and a changed draft rejects the late result',async()=>{
 let finish!:(v:unknown)=>void;let results:unknown[]=[];let validations=0;const revision={current:0};
 const bindings={saving:{current:false},testRevision:revision,mounted:{current:true},profile:null,driver:'mysql',form:{validateFields:()=>{validations++;return new Promise(resolve=>finish=resolve);}},setBusy:()=>{},setTesting:()=>{},setTestResult:(v:unknown)=>results.push(v),invoke:async()=>{},settings:()=>({driver:'mysql',host:'db.test',port:3306,username:'reader',database:'',tls:'disabled'})};
 const testConnection=handler('../src/ConnectionDialog.tsx','testConnection',bindings);const first=testConnection();const duplicate=testConnection();revision.current++;finish({password:'synthetic'});await Promise.all([first,duplicate]);assert.equal(validations,1);assert.ok(!results.some(v=>v&&typeof v==='object'&&(v as {kind:string}).kind==='success'));
});
test('ordinary saved receipt uses a toast while persistence and cleanup warnings remain visible',()=>{
 let notices=0;let toasts=0;const saved=handler('../src/ConnectionManager.tsx','saved',{setDialog:()=>{},setError:()=>{},setNotice:()=>notices++,toast:{success:()=>toasts++},refresh:async()=>{}});
 saved({profile:{},durabilityConfirmed:true,cleanupPending:0});assert.equal(toasts,1);assert.equal(notices,0);
 saved({profile:{},durabilityConfirmed:false,cleanupPending:0});assert.equal(notices,1);assert.equal(toasts,1);
});

for(const failure of [false,true])test(`kube reparse immediately invalidates an old target result even when parsing ${failure?'fails':'succeeds'}`,async()=>{
 let resolve!:(value:unknown)=>void;let reject!:(error:unknown)=>void;let fields={context:'tested-B',namespace:'tested-ns'};let result:unknown={kind:'success',text:'old target success'};let errors='';const revision={current:4};const working={current:false};
 const bindings={working,testRevision:revision,setTestResult:(v:unknown)=>result=v,setBusy:()=>{},setError:(v:string)=>errors=v,changed:{current:0},mounted:{current:true},contextInitialized:{current:true},setPreview:()=>{},form:{setFieldsValue:(v:typeof fields)=>fields=v},parseSource:()=>new Promise((a,b)=>{resolve=a;reject=b;})};
 bindings['invalidateTest' as keyof typeof bindings]=handler('../src/KubeImportDialog.tsx','invalidateTest',bindings) as never;
 const inspect=handler('../src/KubeImportDialog.tsx','inspect',bindings);const pending=inspect();
 assert.equal(result,null);assert.equal(revision.current,5);assert.equal(working.current,true);await inspect();
 if(failure)reject('parse failed');else resolve({settings:{context:'default-A',namespace:'default-ns'}});await pending;
 assert.equal(result,null);assert.equal(working.current,false);
 if(failure)assert.equal(errors,'parse failed');else assert.deepEqual(fields,{context:'default-A',namespace:'default-ns'});
});

for(const mode of ['keep','replace','remove'])test(`database edit test uses current draft with ${mode} credentials without saving`,async()=>{
 const settings={driver:'doris',host:'db.test',port:9030,username:'reader',database:'demo',tls:'disabled'};const calls:{command:string;args:any}[]=[];let result:any;
 const bindings={saving:{current:false},testRevision:{current:0},mounted:{current:true},profile:{id:'saved',revision:7,settings},driver:'doris',mode,credentialTargetUnchanged,form:{validateFields:async(fields:string[])=>{assert.ok(!fields.includes('name'));return {...settings,password:'current-input'};}},settings:()=>settings,setBusy:()=>{},setTesting:()=>{},setTestResult:(v:unknown)=>result=v,invoke:async(command:string,args:unknown)=>calls.push({command,args})};
 await handler('../src/ConnectionDialog.tsx','testConnection',bindings)();assert.equal(calls.length,1);assert.equal(calls[0].command,'profiles_test_db');assert.equal(calls[0].args.request.id,'saved');assert.equal(calls[0].args.request.expectedRevision,7);assert.deepEqual(calls[0].args.request.settings,settings);assert.deepEqual(calls[0].args.request.credential,mode==='replace'?{action:'replacePassword',value:'current-input'}:{action:mode});assert.equal('password'in calls[0].args.request,false);assert.equal(result.kind,'success');
});
test('database edited keep target change is rejected without a backend request',async()=>{
 const original={driver:'doris',host:'old.test',port:9030,username:'reader',database:'',tls:'disabled'};let invoked=0;let result:any;
 const bindings={saving:{current:false},testRevision:{current:0},mounted:{current:true},profile:{id:'saved',revision:1,settings:original},driver:'doris',mode:'keep',credentialTargetUnchanged,form:{validateFields:async()=>({})},settings:()=>({...original,host:'new.test'}),setBusy:()=>{},setTesting:()=>{},setTestResult:(v:unknown)=>result=v,invoke:async()=>invoked++};
 await handler('../src/ConnectionDialog.tsx','testConnection',bindings)();assert.equal(invoked,0);assert.equal(result.kind,'error');assert.match(result.text,/目标.*变化/);
});

test('all-namespace row detail uses the displayed row namespace and exact scope identity',async()=>{
 const scope={sessionId:'A',generation:2,context:'local',catalogRevision:4,resourceId:'pods',namespace:'',search:'',cursor:'',page:0};const shown={scope,data:{catalogRevision:4,scopeRevision:8}};let args:any;let detail:any;
 const bindings={controller:{state:{snapshot:shown,interactive:true}},wantedRef:{current:scope},browserScopeKey,clearDetails:()=>{},detailEpoch:{current:0},selectedSnapshot:{current:null},setSelected:()=>{},setContainer:()=>{},setError:()=>{},setDetail:(v:unknown)=>detail=v,identityArgs:handler('../src/KubePanel.tsx','identityArgs',{}),session:{current:{sessionId:'A',generation:2}},acceptKubeResult,invoke:async(command:string,requested:unknown)=>{assert.equal(command,'kube_resource_detail');args=requested;return {sessionId:'A',generation:2,yaml:'namespace: review',containers:[]};}};
 await handler('../src/KubePanel.tsx','view',bindings)({name:'same-pod',namespace:'review',containers:['app']});assert.equal(args.namespace,'review');assert.equal(args.catalogRevision,4);assert.equal(args.scopeRevision,8);assert.equal(args.resourceId,'pods');assert.equal(detail.yaml,'namespace: review');
});
test('clearing namespace commits an explicit all-scope while preserving current type and search',()=>{
 let value:any={resourceId:'pods',namespace:'ops',search:'late-only',cursor:'third',page:2};let input='';const cursors={current:['','second','third']};
 const change=handler('../src/KubePanel.tsx','changeNamespace',{setNamespaceInput:(v:string)=>input=v,cursors,setLocalPage:()=>{},setPagingNotice:()=>{},setDesired:(update:(v:any)=>any)=>value=update(value)});
 change('');assert.equal(input,'');assert.deepEqual(value,{resourceId:'pods',namespace:'',search:'late-only',cursor:'',page:0});assert.deepEqual(cursors.current,['']);
});

test('changing namespace leaves the displayed local search page untouched before a new snapshot exists',()=>{
 let localPage=2;let desired:any={resourceId:'pods',namespace:'ops',search:'row-',cursor:'',page:0};
 const change=handler('../src/KubePanel.tsx','changeNamespace',{setNamespaceInput:()=>{},cursors:{current:['']},setLocalPage:(v:number)=>localPage=v,setPagingNotice:()=>{},setDesired:(update:(v:any)=>any)=>desired=update(desired)});
 change('denied');assert.equal(localPage,2);assert.equal(desired.namespace,'denied');assert.equal(desired.search,'row-');
});


test('Doris saves using only endpoint and credentials, with an optional database and generated name',async()=>{
 for(const database of ['', 'demo']){
  let requested:Record<string,any>|undefined;let saved=false;
  const bindings={saving:{current:false},setBusy:()=>{},setError:()=>{},form:{validateFields:async()=>({name:'',host:'db.example.test',port:9030,username:'reader',database,password:'synthetic-only',tls:'verifyIdentity'}),setFieldValue:()=>{}},driver:'doris',mode:'replace',profile:null,mounted:{current:true},settings:(v:Record<string,any>)=>({driver:'doris',host:v.host,port:v.port,username:v.username,database:v.database,tls:v.tls}),invoke:async(command:string,args:Record<string,any>)=>{assert.equal(command,'profiles_save');requested=args;return {profile:{}};},onSaved:()=>{saved=true;}};
  await handler('../src/ConnectionDialog.tsx','save',bindings)();
  assert.equal(saved,true);assert.equal(requested?.draft.name,'db.example.test:9030');assert.equal(requested?.draft.settings.database,database);
 }
});


test('Doris hidden TLS input preserves saved credential binding while database remains optional',()=>{
 for(const tls of ['verifyIdentity','required','disabled']){
  const settings=handler('../src/ConnectionDialog.tsx','settings',{driver:'doris',form:{getFieldValue:(field:string)=>field==='tls'?tls:undefined}});
  const draft=settings({host:'db.example.test',port:9030,username:'reader'});
  assert.equal(draft.tls,tls);assert.equal(draft.database,'');
 }
});


test('new MySQL saves endpoint credentials without requiring name, database or a TLS input',async()=>{
 for(const database of ['', 'demo']){
  let requested:Record<string,any>|undefined;
  const bindings={saving:{current:false},setBusy:()=>{},setError:()=>{},form:{validateFields:async()=>({host:'db.example.test',port:3306,username:'reader',database,password:'synthetic-only'}),setFieldValue:()=>{}},driver:'mysql',mode:'replace',profile:null,mounted:{current:true},settings:(v:Record<string,any>)=>({driver:'mysql',host:v.host,port:v.port,username:v.username,database:v.database,tls:'auto'}),invoke:async(_command:string,args:Record<string,any>)=>{requested=args;return {profile:{}};},onSaved:()=>{}};
  await handler('../src/ConnectionDialog.tsx','save',bindings)();
  assert.equal(requested?.draft.name,'db.example.test:3306');assert.equal(requested?.draft.settings.database,database);assert.equal(requested?.draft.settings.tls,'auto');
 }
});

test('a new MySQL draft resolves the hidden automatic TLS value before testing',()=>{
 const settings=handler('../src/ConnectionDialog.tsx','settings',{driver:'mysql',profile:null,form:{getFieldValue:()=>undefined}});
 assert.equal(settings({host:'db.example.test',port:3306,username:'reader'}).tls,'auto');
});

test('collapsing a category leaves the other categories expanded and toggles independently',()=>{
 let state:Record<string,boolean>={};
 const toggle=handler('../src/ConnectionManager.tsx','toggleCategory',{setCollapsedCategories:(change:(v:Record<string,boolean>)=>Record<string,boolean>)=>state=change(state)});
 toggle('数据库');assert.equal(state['数据库'],true);assert.equal(state['Kubernetes'],undefined);
 toggle('主机与终端');assert.equal(state['数据库'],true);assert.equal(state['主机与终端'],true);
 toggle('数据库');assert.equal(state['数据库'],false);assert.equal(state['主机与终端'],true);
});

test('MinIO page results cannot replace a newer bucket or directory selection',async()=>{
 let complete!:(value:unknown)=>void;let shown:any=null;let busy=false;
 const wanted={current:JSON.stringify(['photos','',''])};const generation={current:0};
 const load=handler('../src/MinioPanel.tsx','loadPage',{session:{id:'runtime'},mounted:{current:true},loadGeneration:generation,wantedRef:wanted,minioScopeKey:(...parts:string[])=>JSON.stringify(parts),setLoading:(v:boolean)=>busy=v,setError:()=>{},setPage:(v:unknown)=>shown=v,invoke:()=>new Promise(resolve=>complete=resolve)});
 const pending=load('photos','','');wanted.current=JSON.stringify(['documents','','']);complete({objects:[{key:'old.jpg'}],nextCursor:null});await pending;assert.equal(shown,null);
 const current=load('documents','','');complete({objects:[{key:'current.txt'}],nextCursor:null});await current;assert.equal(shown.objects[0].key,'current.txt');assert.equal(busy,false);
});
test('MinIO save stores the complete key pair in credential input, clears it after success and never copies it into metadata',async()=>{
 let sent:any;let cleared:any;let saved=false;
 const values={name:'',endpoint:'http://localhost:9000',region:'us-east-1',bucket:'photos',accessKey:'synthetic-access',secretKey:'synthetic-secret'};
 const bindings={form:{validateFields:async()=>values,setFieldsValue:(v:unknown)=>cleared=v},settings:(v:any)=>({driver:'minio',endpoint:v.endpoint,region:v.region,bucket:v.bucket}),profile:null,mode:'replace',saving:{current:false},mounted:{current:true},setBusy:()=>{},setError:()=>{},credentialTargetUnchanged,invoke:async(_command:string,args:unknown)=>{sent=args;return {profile:{id:'saved'},durabilityConfirmed:true,cleanupPending:0};},onSaved:()=>saved=true};
 await handler('../src/MinioDialog.tsx','save',bindings)();assert.equal(saved,true);assert.equal(sent.credential.action,'replaceMinio');assert.equal(sent.credential.accessKey,'synthetic-access');assert.equal(sent.credential.secretKey,'synthetic-secret');assert.equal(JSON.stringify(sent.draft).includes('synthetic-'),false);assert.deepEqual(cleared,{accessKey:'',secretKey:''});
});
test('declining overwrite never starts an overwriting upload',async()=>{
 const requests:any[]=[];
 const upload=handler('../src/MinioPanel.tsx','uploadOne',{session:{id:'runtime'},uploadFileName:()=> 'report.txt',askOverwrite:async()=>false,invoke:async(_command:string,args:any)=>{requests.push(args);throw 'OBJECT_EXISTS';}});
 assert.equal(await upload('/fixture/report.txt','photos','folder/'),null);assert.equal(requests.length,1);assert.equal(requests[0].overwrite,false);assert.equal(requests[0].prefix,'folder/');
});
