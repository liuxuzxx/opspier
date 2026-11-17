import test from 'node:test';import assert from 'node:assert/strict';
import {KubeTerminalController} from '../src/kube-terminal.ts';
const identity={sessionId:'owned-session',generation:1};
const tick=()=>new Promise(resolve=>setImmediate(resolve));
test('terminal close during startup records the original identity and a late open is closed again',async()=>{
 const calls:{command:string;args:any}[]=[];let complete!:(v:any)=>void;
 const controller=new KubeTerminalController(identity,'',async(command,args)=>{calls.push({command,args});if(command==='terminal_open')return new Promise(resolve=>complete=resolve);},()=>{});
 const opening=controller.open('','');const scope=calls[0].args.scope;await controller.close();complete(scope);await opening;
 assert.equal(controller.state.status,'closed');assert.ok(calls.filter(c=>c.command==='terminal_close').every(c=>c.args.scope.terminalId===scope.terminalId));assert.equal(calls.filter(c=>c.command==='terminal_close').length,2);
});
test('hiding preserves the terminal and context replacement requires an explicit open',async()=>{
 const calls:string[]=[];const invoke=async(command:string,args:any)=>{calls.push(command);return command==='terminal_open'?args.scope:{data:[],done:false,queuedBytes:0};};
 const controller=new KubeTerminalController(identity,'first',invoke,()=>{});await controller.open('','');const scope=controller.state.scope;controller.setVisible(false);controller.setVisible(true);assert.equal(controller.state.scope,scope);assert.equal(calls.filter(c=>c==='terminal_open').length,1);
 controller.dispose();const next=new KubeTerminalController({...identity,generation:2},'',invoke,()=>{});assert.equal(next.state.status,'closed');assert.equal(calls.filter(c=>c==='terminal_open').length,1);
});
test('namespace changes are serialized config writes and input waits for the most recent default',async()=>{
 const calls:{command:string;args:any}[]=[];let complete!:(v:any)=>void;
 const controller=new KubeTerminalController(identity,'first',async(command,args)=>{calls.push({command,args});if(command==='terminal_open')return args.scope;if(command==='terminal_namespace'&&args.namespace==='second')return new Promise(resolve=>complete=resolve);},()=>{});
 await controller.open('','');const changing=controller.namespace('second');assert.equal(controller.canInput,false);assert.equal(controller.send('must-not-run'),false);void controller.namespace('');complete(undefined);await changing;await tick();
 assert.deepEqual(calls.filter(c=>c.command==='terminal_namespace').map(c=>c.args.namespace),['second','']);assert.equal(controller.canInput,true);assert.equal(calls.some(c=>c.command==='terminal_input'),false);
});
test('a late poll cannot write into a replacement terminal and oversized input is rejected',async()=>{
 let complete!:(v:any)=>void;const controller=new KubeTerminalController(identity,'',async(command,args)=>{if(command==='terminal_open')return args.scope;if(command==='terminal_poll')return new Promise(resolve=>complete=resolve);},()=>{});
 await controller.open('','');assert.equal(controller.send('x'.repeat(65537)),false);const pending=controller.poll();await controller.close();await controller.open('','');complete({data:[42],done:false,queuedBytes:0});assert.equal(await pending,null);
});
test('suspended context switching blocks queued keystrokes and preserves the prior shell on failure',async()=>{
 const controller=new KubeTerminalController(identity,'',async(command,args)=>command==='terminal_open'?args.scope:undefined,()=>{});await controller.open('','');controller.setSuspended(true);assert.equal(controller.send('bad'),false);controller.setSuspended(false);assert.equal(controller.canInput,true);controller.dispose();
});
test('reopening an exited shell first closes its prior reservation',async()=>{
 const calls:{command:string;args:any}[]=[];const controller=new KubeTerminalController(identity,'',async(command,args)=>{calls.push({command,args});return command==='terminal_open'?args.scope:command==='terminal_poll'?{data:[],done:true,queuedBytes:0}:undefined;},()=>{});
 await controller.open('','');const old=controller.state.scope;await controller.poll();await controller.open('','');const last=calls.filter(c=>c.command==='terminal_close');assert.equal(last.length,1);assert.deepEqual(last[0].args.scope,old);assert.equal(controller.state.status,'connected');
});
test('a delayed input from the old shell cannot strand the new shell input queue',async()=>{
 let oldComplete!:(v:any)=>void;let first=true;const sent:number[][]=[];
 const controller=new KubeTerminalController(identity,'',async(command,args)=>{if(command==='terminal_open')return args.scope;if(command==='terminal_input'){sent.push(args.data);if(first){first=false;return new Promise(resolve=>oldComplete=resolve);}}},()=>{});
 await controller.open('','');controller.send('old');await controller.close();await controller.open('','');controller.send('new');oldComplete(undefined);await tick();await tick();assert.deepEqual(sent.map(data=>new TextDecoder().decode(new Uint8Array(data))),['old','new']);controller.dispose();
});
test('a real KubeSelection is narrowed before crossing the strict terminal IPC boundary',async()=>{
 let sent:any;const selection={...identity,context:'same-context',namespace:'first',contexts:['same-context','second-context']};
 const controller=new KubeTerminalController(selection,'first',async(command,args)=>{if(command==='terminal_open'){sent=args.scope;return args.scope;}},()=>{});await controller.open('','');assert.deepEqual(Object.keys(sent).sort(),['generation','revision','sessionId','terminalId']);controller.dispose();
});

test('failed terminal input stops queued suffixes before they can execute an incomplete command',async()=>{
 let fail!:(error:Error)=>void;const inputs:string[]=[];const closed:any[]=[];
 const controller=new KubeTerminalController(identity,'',async(command,args)=>{
  if(command==='terminal_open')return args.scope;
  if(command==='terminal_input'){inputs.push(new TextDecoder().decode(new Uint8Array(args.data)));return new Promise((_resolve,reject)=>fail=reject);}
  if(command==='terminal_close')closed.push(args.scope);
 },()=>{});
 await controller.open('','');const scope=controller.state.scope;
 assert.equal(controller.send('prefix-'),true);assert.equal(controller.send('suffix\r'),true);
 fail(Error('owned input queue is full'));await tick();await tick();
 assert.deepEqual(inputs,['prefix-']);assert.equal(controller.canInput,false);
 assert.equal(controller.state.status,'error');assert.ok(closed.some(value=>value.terminalId===scope!.terminalId));
 controller.dispose();
});

test('an old terminal input rejection cannot stop a replacement terminal or drop its queued input',async()=>{
 let fail!:(error:Error)=>void;let first=true;const inputs:string[]=[];
 const controller=new KubeTerminalController(identity,'',async(command,args)=>{
  if(command==='terminal_open')return args.scope;
  if(command==='terminal_input'){inputs.push(new TextDecoder().decode(new Uint8Array(args.data)));if(first){first=false;return new Promise((_resolve,reject)=>fail=reject);}}
 },()=>{});
 await controller.open('','');controller.send('old');await controller.close();await controller.open('','');
 controller.send('new');fail(Error('old session ended'));await tick();await tick();
 assert.deepEqual(inputs,['old','new']);assert.equal(controller.state.status,'connected');controller.dispose();
});

test('accepted command fragments drain before a namespace update can change their default',async()=>{
 let finishInput!:(value?:unknown)=>void;let finishNamespace!:(value?:unknown)=>void;const events:string[]=[];
 const controller=new KubeTerminalController(identity,'first',async(command,args)=>{
  if(command==='terminal_open')return args.scope;
  if(command==='terminal_input'){
   const text=new TextDecoder().decode(new Uint8Array(args.data));events.push('input:'+text);
   if(text==='kubectl get ')return new Promise(resolve=>finishInput=resolve);
  }
  if(command==='terminal_namespace'){events.push('namespace:'+args.namespace);return new Promise(resolve=>finishNamespace=resolve);}
 },()=>{});
 try{
  await controller.open('','');
  assert.equal(controller.send('kubectl get '),true);assert.equal(controller.send('pods\r'),true);
  const changing=controller.namespace('second');
  assert.equal(controller.canInput,false);assert.deepEqual(events,['input:kubectl get ']);
  finishInput();await tick();
  assert.deepEqual(events,['input:kubectl get ','input:pods\r','namespace:second']);
  assert.equal(controller.canInput,false);
  finishNamespace();await changing;await tick();
  assert.equal(controller.state.appliedNamespace,'second');assert.equal(controller.canInput,true);
 }finally{controller.dispose();}
});

test('context suspension preserves accepted command suffixes and resumes them when the switch fails',async()=>{
 let finishInput!:(value?:unknown)=>void;const inputs:string[]=[];
 const controller=new KubeTerminalController(identity,'first',async(command,args)=>{
  if(command==='terminal_open')return args.scope;
  if(command==='terminal_input'){
   const text=new TextDecoder().decode(new Uint8Array(args.data));inputs.push(text);
   if(text==='kubectl get ')return new Promise(resolve=>finishInput=resolve);
  }
 },()=>{});
 try{
  await controller.open('','');controller.send('kubectl get ');controller.send('pods\r');
  controller.setSuspended(true);finishInput();await tick();
  assert.deepEqual(inputs,['kubectl get ']);assert.equal(controller.canInput,false);
  controller.setSuspended(false);await tick();
  assert.deepEqual(inputs,['kubectl get ','pods\r']);
  assert.equal(controller.state.status,'connected');assert.equal(controller.canInput,true);
 }finally{controller.dispose();}
});
