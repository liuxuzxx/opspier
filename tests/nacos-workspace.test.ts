import test from 'node:test';import assert from 'node:assert/strict';
const deferred=<T>()=>{let resolve!:(value:T)=>void;let reject!:(reason:unknown)=>void;const promise=new Promise<T>((a,b)=>{resolve=a;reject=b;});return{promise,resolve,reject};};
const module=await import('../src/nacos-workspace.ts').catch((error:NodeJS.ErrnoException)=>{if(error.code==='ERR_MODULE_NOT_FOUND')return {};throw error;});
const create=()=>{assert.equal(typeof module.NacosWorkspace,'function','production Nacos request gate must exist');return new module.NacosWorkspace();};
test('late queries cannot overwrite another Nacos target or a disposed workspace',async()=>{
 const gate=create();await gate.transition('namespace-a/config-a');const old=deferred<string>();const read=gate.query(()=>old.promise);
 await gate.transition('namespace-b/config-b');old.resolve('old-content');assert.equal(await read,undefined);
 const next=deferred<string>();const pending=gate.query(()=>next.promise);gate.dispose();next.resolve('next-content');assert.equal(await pending,undefined);
});
test('mutation confirmation locks target changes and duplicate writes synchronously',async()=>{
 const gate=create();await gate.transition('a');const confirmation=deferred<boolean>();const network=deferred<string>();let calls=0;
 const first=gate.mutate(()=>confirmation.promise,()=>{calls++;return network.promise;});assert.equal(gate.locked,true);
 assert.equal(await gate.transition('b'),false);assert.equal(await gate.mutate(async()=>true,async()=>{calls++;return 'duplicate';}),undefined);
 confirmation.resolve(true);await Promise.resolve();assert.equal(calls,1);network.resolve('verified');assert.equal(await first,'verified');assert.equal(gate.locked,false);
});
test('cancelled dirty-draft navigation and rejected writes retain the original target',async()=>{
 const gate=create();await gate.transition('old');assert.equal(await gate.transition('new',async()=>false),false);assert.equal(gate.scope,'old');
 let writes=0;assert.equal(await gate.mutate(async()=>false,async()=>{writes++;return 'bad';}),undefined);assert.equal(writes,0);
 await assert.rejects(gate.mutate(async()=>true,async()=>{throw Error('not verified');}),/not verified/);assert.equal(gate.scope,'old');assert.equal(gate.locked,false);
});
test('an unmounted Nacos write never applies its eventual verified result',async()=>{
 const gate=create();await gate.transition('a');const network=deferred<string>();const write=gate.mutate(async()=>true,()=>network.promise);await Promise.resolve();gate.dispose();network.resolve('late');assert.equal(await write,undefined);
});
