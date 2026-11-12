import assert from 'node:assert/strict';
import test from 'node:test';
import { metadata, OutputGate } from '../src/model.ts';
import * as display from '../src/model.ts';
test('saved connections contain only explicit public metadata', () => {
 assert.deepEqual(metadata({id:'a',kind:'ssh',name:'prod',host:'localhost',port:22,username:'u',password:'secret',privateKey:'private',passphrase:'phrase'}),{id:'a',kind:'ssh',name:'prod',host:'localhost',port:22,username:'u'});
});
test('output queue is bounded and discards stale generations', () => {
 const gate = new OutputGate(8); const old = gate.next();
 assert.equal(gate.push(old, new Uint8Array([1,2,3,4,5])),true);
 assert.equal(gate.push(old,new Uint8Array([6,7,8,9])),false);
 assert.deepEqual([...gate.drain()],[1,2,3,4,5]);
 const next = gate.next(); assert.equal(gate.push(old,new Uint8Array([0])),false);
 assert.equal(gate.push(next,new Uint8Array([9])),true);
 assert.deepEqual([...gate.drain()],[9]);
});
test('connection labels keep the original domain and bracket IPv6 without credentials',()=>{
 const format=(display as any).formatEndpoint;
 assert.equal(format?.('reader','db.example.test',3306),'reader@db.example.test:3306');
 assert.equal(format?.('ops','2001:db8::1',22),'ops@[2001:db8::1]:22');
 assert.equal(format?.('ops','[::1]',2222),'ops@[::1]:2222');
});
test('terminal states expose semantic color and a concise public label',()=>{
 const state=(display as any).connectionState;
 assert.deepEqual(state?.('connected'),{color:'green',label:'已连接'});
 assert.deepEqual(state?.('connecting'),{color:'blue',label:'连接中'});
 assert.deepEqual(state?.('disconnected'),{color:'default',label:'已断开'});
 assert.deepEqual(state?.('SSH输出连接中断'),{color:'red',label:'连接异常'});
});
