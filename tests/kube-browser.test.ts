import {test} from 'node:test';
import assert from 'node:assert/strict';
import {KubeBrowser, type BrowserScope} from '../src/kube-browser.ts';
const scope:BrowserScope={sessionId:'A',generation:1,context:'local',catalogRevision:0,resourceId:'pods',namespace:'ops',search:'',cursor:'',page:0};
function page(id:number,namespace='ops',name='old') {return {sessionId:'A',generation:1,requestId:id,catalogRevision:0,scopeRevision:1,resourceId:'pods',namespace,search:'',items:[{name,namespace,kind:'pods',status:'Running',created:'',containers:[]}],continueToken:'next',resourceVersion:'42',scanned:1,pages:1,bytes:80,truncated:false,reason:null};}
function setup(){const pending=new Map<number,{resolve:(v:any)=>void;reject:(e:unknown)=>void}>();const cancelled:number[]=[];const states:any[]=[];const browser=new KubeBrowser(async(_scope,id)=>new Promise((resolve,reject)=>pending.set(id,{resolve,reject})),async(_scope,id)=>{cancelled.push(id);},state=>states.push(state));return {browser,pending,cancelled,states};}
test('query keeps the entire old snapshot until the latest successful scope replaces it atomically',async()=>{
 const {browser,pending}=setup();const first=browser.run(scope);pending.get(1)!.resolve(page(1));await first;const old=browser.state.snapshot;
 const second=browser.run({...scope,namespace:'review',page:1,cursor:'next'});assert.equal(browser.state.snapshot,old);assert.equal(browser.state.interactive,false);
 pending.get(2)!.resolve(page(2,'review','new'));await second;assert.equal(browser.state.snapshot?.scope.page,1);assert.equal(browser.state.snapshot?.data.items[0].name,'new');assert.equal(browser.state.interactive,true);
});
test('failed scope change preserves old rows, cursor and displayed scope but forbids their actions',async()=>{
 const {browser,pending}=setup();const first=browser.run(scope);pending.get(1)!.resolve(page(1));await first;const old=browser.state.snapshot;
 const second=browser.run({...scope,namespace:'denied',cursor:'new-cursor',page:2});pending.get(2)!.reject('403 read access denied');await second;
 assert.equal(browser.state.snapshot,old);assert.equal(browser.state.snapshot?.scope.namespace,'ops');assert.equal(browser.state.snapshot?.scope.page,0);assert.equal(browser.state.interactive,false);assert.match(browser.state.error,/403/);
});
test('new request cancels the old one and its late completion cannot replace data or clear new pending state',async()=>{
 const {browser,pending,cancelled}=setup();const first=browser.run(scope);const second=browser.run({...scope,namespace:'review'});assert.deepEqual(cancelled,[1]);pending.get(1)!.resolve(page(1));await first;
 assert.equal(browser.state.snapshot,null);assert.equal(browser.state.pending?.namespace,'review');pending.get(2)!.resolve(page(2,'review'));await second;assert.equal(browser.state.snapshot?.scope.namespace,'review');
});
test('background refresh does not overlap or invalidate rows and its failure retains an interactive snapshot',async()=>{
 const {browser,pending}=setup();const first=browser.run(scope);pending.get(1)!.resolve(page(1));await first;const old=browser.state.snapshot;
 const refresh=browser.run(scope,true);await browser.run(scope,true);assert.equal(pending.size,2);assert.equal(browser.state.snapshot,old);assert.equal(browser.state.interactive,true);
 pending.get(2)!.reject('network unavailable');await refresh;assert.equal(browser.state.snapshot,old);assert.equal(browser.state.interactive,true);
});
test('generation and catalogue mismatch cannot publish a snapshot and dispose cancels pending transport',async()=>{
 const {browser,pending,cancelled}=setup();const request=browser.run(scope);pending.get(1)!.resolve({...page(1),generation:2});await request;assert.equal(browser.state.snapshot,null);
 const another=browser.run(scope);browser.dispose();assert.deepEqual(cancelled,[2]);pending.get(2)!.resolve(page(2));await another;assert.equal(browser.state.snapshot,null);
});

test('local search page belongs to displayed snapshot through pending and rejected scope changes',async()=>{
 const {browser,pending}=setup();const searching={...scope,search:'row-'};
 const rows=Array.from({length:250},(_,i)=>({name:`row-${i+1}`,namespace:'ops',kind:'pods',status:'Running',created:'',containers:[]}));
 const first=browser.run(searching);pending.get(1)!.resolve({...page(1),search:'row-',items:rows,truncated:true,reason:'old bounded snapshot'});await first;
 browser.setLocalPage(2);const old=browser.state.snapshot;assert.equal(old?.localPage,2);assert.equal(old?.data.items[200].name,'row-201');
 const failed=browser.run({...searching,namespace:'denied'});assert.equal(browser.state.snapshot,old);pending.get(2)!.reject('403');await failed;assert.equal(browser.state.snapshot,old);assert.equal(browser.state.snapshot?.localPage,2);assert.equal(browser.state.snapshot?.data.reason,'old bounded snapshot');
 const success=browser.run({...searching,namespace:'review'});assert.equal(browser.state.snapshot,old);pending.get(3)!.resolve({...page(3,'review','new-row'),search:'row-'});await success;assert.equal(browser.state.snapshot?.localPage,0);assert.equal(browser.state.snapshot?.scope.namespace,'review');
});
test('same-scope search refresh clamps the displayed page only with the successful new rows',async()=>{
 const {browser,pending}=setup();const searching={...scope,search:'row-'};const rows=Array.from({length:250},(_,i)=>({name:`row-${i+1}`,namespace:'ops',kind:'pods',status:'Running',created:'',containers:[]}));
 const first=browser.run(searching);pending.get(1)!.resolve({...page(1),search:'row-',items:rows});await first;browser.setLocalPage(2);
 const refresh=browser.run(searching,true);assert.equal(browser.state.snapshot?.localPage,2);pending.get(2)!.resolve({...page(2),search:'row-',items:rows.slice(0,110)});await refresh;assert.equal(browser.state.snapshot?.localPage,1);assert.equal(browser.state.snapshot?.data.items.length,110);
});
