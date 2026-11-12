import {test} from 'node:test';
import assert from 'node:assert/strict';
import {initialConnectionValues} from '../src/connection-draft.ts';
test('new connection ports follow explicit drivers and editing preserves the supplied port',()=>{
 assert.equal(initialConnectionValues('doris',null).port,9030);
 assert.equal(initialConnectionValues('mysql',null).port,3306);
 assert.equal(initialConnectionValues('ssh',null).port,22);
 assert.equal(initialConnectionValues('doris',{port:19030}).port,19030);
 assert.equal(initialConnectionValues('mysql',{port:13306}).port,13306);
 assert.equal(initialConnectionValues('doris',{port:0}).port,0,'invalid input must be validated, not silently replaced');
});

test('new MySQL defaults to automatic TLS while existing transport choices are preserved',()=>{
 assert.equal(initialConnectionValues('mysql',null).tls,'auto');
 for(const tls of ['verifyIdentity','required','disabled'] as const){
  assert.equal(initialConnectionValues('mysql',{tls}).tls,tls);
 }
 assert.equal(initialConnectionValues('doris',null).port,9030);
});
