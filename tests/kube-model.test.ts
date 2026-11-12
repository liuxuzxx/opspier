import { test } from 'node:test';
import assert from 'node:assert/strict';
import { RESOURCE_KINDS, pageLimit, acceptGeneration, appendLog, searchRows } from '../src/kube-model.ts';
import * as kube from '../src/kube-model.ts';

test('only supported read-only resource kinds appear in navigation', () => {
  assert.deepEqual(RESOURCE_KINDS, ['pods','deployments','statefulsets','daemonsets','jobs','cronjobs','services','configmaps','secrets','persistentvolumeclaims','persistentvolumes','events']);
});
test('page limits are finite integers between 1 and 200', () => {
  assert.equal(pageLimit(99999), 200);
  assert.equal(pageLimit(-1), 1);
  assert.equal(pageLimit(Number.NaN), 100);
  assert.equal(pageLimit(12.8), 12);
});
test('a switched context rejects stale and future generation output', () => {
  assert.equal(acceptGeneration(2, 1), false);
  assert.equal(acceptGeneration(2, 3), false);
  assert.equal(acceptGeneration(2, 2), true);
});
test('log append retains bounded tail with explicit dropped count', () => {
  assert.deepEqual(appendLog('abcd', 'efgh', 5), { text:'defgh', dropped:3 });
  assert.deepEqual(appendLog('', '😀😀', 1), { text:'😀', dropped:1 });
});
test('case-insensitive search spans resource name, namespace, status', () => {
  const rows = [{name:'api',namespace:'prod',status:'Running'}, {name:'db',namespace:'test',status:'Pending'}];
  assert.deepEqual(searchRows(rows, 'PROD'), [rows[0]]);
  assert.deepEqual(searchRows(rows, 'pending'), [rows[1]]);
});

test('watch updates, removes and inserts resource identities without growing beyond the page', () => {
  const row = (name:string, status='Pending', namespace='ops') => ({name,namespace,status,kind:'pods',created:'',containers:[]});
  const result = kube.applyWatchChanges([row('a'),row('b'),row('a','Running','other')], [
    {event:'MODIFIED',row:row('a','Running')},
    {event:'DELETED',row:row('b')},
    {event:'ADDED',row:row('c')},
  ],3);
  assert.deepEqual(result.items.map(r=>[r.namespace,r.name,r.status]), [['ops','a','Running'],['other','a','Running'],['ops','c','Pending']]);
  assert.equal(result.refresh,false);
  const overflow=kube.applyWatchChanges([row('a')],[{event:'ADDED',row:row('b')}],1);
  assert.deepEqual(overflow.items.map(r=>r.name),['a']);
  assert.equal(overflow.refresh,true);
});

test('same generation from another runtime is rejected',async()=>{
 const {acceptKubeResult}=await import('../src/kube-model.ts');
 assert.equal(acceptKubeResult({sessionId:'A',generation:1},{sessionId:'B',generation:1}),false);
 assert.equal(acceptKubeResult({sessionId:'A',generation:1},{sessionId:'A',generation:2}),false);
 assert.equal(acceptKubeResult({sessionId:'A',generation:1},{sessionId:'A',generation:1}),true);
 assert.equal(acceptKubeResult(null,{sessionId:'A',generation:1}),false);
});

test('resource status colors distinguish success, waiting, failure and unknown without rewriting status',()=>{
 const color=(kube as any).statusColor;
 assert.equal(color('Running'),'green');assert.equal(color('Succeeded'),'green');assert.equal(color('Pending'),'gold');assert.equal(color('Failed'),'red');assert.equal(color('CrashLoopBackOff'),'red');assert.equal(color('Unknown'),'default');assert.equal(color('custom-progress'),'default');
});
test('namespace color is stable across refreshes and differentiates fixture namespaces',()=>{
 const color=(kube as any).namespaceColor;const ops=color('ops');assert.equal(color('ops'),ops);assert.notEqual(color('review'),ops);assert.equal(color(''),'default');
});
