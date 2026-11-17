import test from 'node:test';import assert from 'node:assert/strict';import {readFileSync} from 'node:fs';import ts from 'typescript';
import {credentialTargetUnchanged} from '../src/profiles-model.ts';import {NacosWorkspace} from '../src/nacos-workspace.ts';import {nacosScopeKey} from '../src/nacos-model.ts';
function handler(file:string,name:string,bindings:Record<string,unknown>){
 const source=ts.createSourceFile(file,readFileSync(new URL(file,import.meta.url),'utf8'),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let node:ts.Node|undefined;
 const visit=(value:ts.Node)=>{if(ts.isFunctionDeclaration(value)&&value.name?.text===name)node=value;ts.forEachChild(value,visit);};visit(source);assert.ok(node);
 const js=ts.transpileModule(`const selected=${node.getText(source)};`,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.None}}).outputText;
 return new Function(...Object.keys(bindings),js+';return selected;')(...Object.values(bindings));
}
function dialog(overrides:Record<string,unknown>={}){
 const value={name:'fixture',endpoint:'http://127.0.0.1:8848/nacos',authentication:'password',namespace:'',username:'synthetic-user',password:'synthetic-password'};const calls:{command:string;args:unknown}[]=[];let result:unknown;let cleared=false;let saved=false;
 const bindings={occupied:{current:false},revision:{current:0},mounted:{current:true},profile:null,mode:'replace',setBusy:()=>{},setTesting:()=>{},setResult:(v:unknown)=>result=v,setError:()=>{},credentialTargetUnchanged,
 form:{validateFields:async()=>value,setFieldsValue:()=>cleared=true},onSaved:()=>saved=true,invoke:async(command:string,args:unknown)=>{calls.push({command,args});return {profile:{id:'saved'},durabilityConfirmed:true,cleanupPending:0};},...overrides};
 Object.assign(bindings,{settings:handler('../src/NacosDialog.tsx','settings',{}),credential:handler('../src/NacosDialog.tsx','credential',{mode:bindings.mode})});
 return{value,bindings,calls,snapshot:()=>({result,cleared,saved})};
}
test('Nacos test uses draft credential action and never saves or opens a runtime',async()=>{
 const d=dialog();await handler('../src/NacosDialog.tsx','testConnection',d.bindings)();assert.equal(d.calls.length,1);assert.equal(d.calls[0].command,'profiles_test_nacos');assert.equal((d.calls[0].args as any).request.credential.action,'replaceNacos');assert.equal(d.snapshot().cleared,false);assert.equal(d.snapshot().saved,false);
});
test('Nacos target revision changes during field validation prevent authentication requests',async()=>{
 let complete!:(value:unknown)=>void;const d=dialog({form:{validateFields:()=>new Promise(resolve=>complete=resolve),setFieldsValue:()=>{}}});
 const promise=handler('../src/NacosDialog.tsx','testConnection',d.bindings)();d.bindings.revision.current++;complete(d.value);await promise;assert.equal(d.calls.length,0);
});
test('Nacos anonymous tests send no invented password and Keep refuses another target',async()=>{
 const a=dialog();a.value.authentication='none';await handler('../src/NacosDialog.tsx','testConnection',a.bindings)();assert.deepEqual((a.calls[0].args as any).request.credential,{action:'remove'});
 const d=dialog({profile:{id:'old',revision:1,settings:{driver:'nacos',endpoint:'http://127.0.0.1:9999/nacos',authentication:'password',namespace:''}},mode:'keep'});await handler('../src/NacosDialog.tsx','testConnection',d.bindings)();assert.equal(d.calls.length,0);
});
test('failed Nacos saves retain current input, while committed saves clear it',async()=>{
 const failed=dialog({invoke:async()=>{throw Error('credential provider unavailable');}});await handler('../src/NacosDialog.tsx','save',failed.bindings)();assert.equal(failed.snapshot().cleared,false);assert.equal(failed.snapshot().saved,false);
 const good=dialog();await handler('../src/NacosDialog.tsx','save',good.bindings)();assert.equal(good.snapshot().cleared,true);assert.equal(good.snapshot().saved,true);
});
test('Nacos save and test cannot overlap before validation completes',async()=>{
 let complete!:(value:unknown)=>void;const d=dialog({form:{validateFields:()=>new Promise(resolve=>complete=resolve),setFieldsValue:()=>{}}});
 const testConnection=handler('../src/NacosDialog.tsx','testConnection',d.bindings);const save=handler('../src/NacosDialog.tsx','save',d.bindings);const promise=testConnection();await save();complete(d.value);await promise;assert.equal(d.calls.length,1);assert.equal(d.calls[0].command,'profiles_test_nacos');
});
test('instance draft confirmation locks the current Nacos service until resolved',async()=>{
 const gate=new NacosWorkspace();await gate.transition('service-A');let complete!:(v:boolean)=>void;let selected=0;
 const bindings={gate,instance:{enabled:true,weight:1,metadata:{}},enabled:false,weight:1,instanceMetadata:'{}',json:(v:unknown)=>JSON.stringify(v),confirm:()=>new Promise(resolve=>complete=resolve),mounted:{current:true},serviceView:{key:{namespaceId:'a',group:'g',name:'service'}},nacosScopeKey,setInstance:()=>selected++,setEnabled:()=>{},setWeight:()=>{},setInstanceMetadata:()=>{}};
 const pending=handler('../src/NacosPanel.tsx','selectInstance',bindings)({key:{ip:'127.0.0.1',port:1,cluster:'DEFAULT',ephemeral:true},enabled:true,weight:1,metadata:{}});
 assert.equal(await gate.transition('service-B'),false);complete(true);await pending;await new Promise(resolve=>setImmediate(resolve));assert.equal(selected,1);
});
test('closing a dirty Nacos workspace waits for draft confirmation and cancellation keeps the runtime',async()=>{
 let closed=0,removed=0;const workspace={id:'owned',kind:'nacos'};const bindings={closeChecks:{current:new Map([['owned',async()=>false]])},closeRuntime:async()=>closed++,removeWorkspace:()=>removed++,setError:()=>{}};
 await handler('../src/ConnectionManager.tsx','closeWorkspace',bindings)(workspace);assert.equal(closed,0);assert.equal(removed,0);
 bindings.closeChecks.current.set('owned',async()=>true);await handler('../src/ConnectionManager.tsx','closeWorkspace',bindings)(workspace);assert.equal(closed,1);assert.equal(removed,1);
});
test('slow config details lock editor publication until the matching read completes',async()=>{
 const gate=new NacosWorkspace();let complete!:(v:any)=>void;let loading=false,writes=0;const detailPending={current:false},detailEpoch={current:0};const key={namespaceId:'a',group:'g',dataId:'file'};
 const bindings={gate,detailPending,detailEpoch,setDetailLoading:(v:boolean)=>loading=v,mounted:{current:true},nacosScopeKey,leave:async()=>true,clear:()=>{},setKey:()=>{},setOriginal:()=>{},setContent:()=>{},setConfigType:()=>{},setError:()=>{},session:{id:'s'},invoke:()=>new Promise(resolve=>complete=resolve),key,original:null,content:'draft',configType:'yaml',describe:()=>'',write:()=>writes++};
 const pending=handler('../src/NacosPanel.tsx','chooseConfig',bindings)(key);await new Promise(resolve=>setImmediate(resolve));assert.equal(detailPending.current,true);assert.equal(loading,true);handler('../src/NacosPanel.tsx','publish',bindings)();assert.equal(writes,0);
 complete({content:'server',type:'yaml'});await pending;assert.equal(detailPending.current,false);assert.equal(loading,false);
});
test('updating service attributes preserves an existing unpublished instance draft',()=>{
 let selected:any={enabled:true};const bindings={serviceView:{key:{namespaceId:'',group:'g',name:'api'}},serviceMetadata:'{}',threshold:.3,parseMetadata:JSON.parse,describe:()=>'',session:{id:'s'},invoke:()=>{},json:JSON.stringify,setServiceView:()=>{},setThreshold:()=>{},setServiceMetadata:()=>{},setInstance:(value:any)=>selected=value,setError:()=>{},write:(_title:any,_target:any,_work:any,apply:any)=>apply({protectThreshold:.3,metadata:{}})};
 handler('../src/NacosPanel.tsx','saveService',bindings)();assert.notEqual(selected,null);
});
