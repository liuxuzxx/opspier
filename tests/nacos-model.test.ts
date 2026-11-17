import test from 'node:test';import assert from 'node:assert/strict';
import {nacosScopeKey,parseMetadata,nacosDefaults,requiresCredential,configDirty,canApplyNacosResult} from '../src/nacos-model.ts';
test('Nacos scope uses namespace IDs, group and dataId without string collision',()=>{
 assert.notEqual(nacosScopeKey('a','bc','d'),nacosScopeKey('ab','c','d'));
 assert.equal(nacosScopeKey('public','g','x'),nacosScopeKey('','g','x'));
 assert.equal(canApplyNacosResult('A','A',3,3),true);assert.equal(canApplyNacosResult('A','B',3,3),false);assert.equal(canApplyNacosResult('A','A',2,3),false);
});
test('metadata editor accepts string maps and rejects arrays, numbers and nested values',()=>{
 assert.deepEqual(parseMetadata('{"zone":"blue","empty":""}'),{zone:'blue',empty:''});
 for(const value of ['[]','null','{"weight":1}','{"nested":{}}','{bad'])assert.throws(()=>parseMetadata(value));
});
test('anonymous Nacos opens without inventing credentials and draft content preserves whitespace',()=>{
 assert.equal(requiresCredential({driver:'nacos',authentication:'none'}),false);assert.equal(requiresCredential({driver:'nacos',authentication:'password'}),true);assert.equal(requiresCredential({driver:'mysql'}),true);
 assert.equal(nacosDefaults().endpoint,'http://localhost:8848/nacos');
 assert.equal(configDirty('a\n','a\n'),false);assert.equal(configDirty('a\n','a'),true);
});
