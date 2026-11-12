import test from 'node:test';
import assert from 'node:assert/strict';
import { categoryFor, credentialTargetUnchanged, legacyProfiles } from '../src/profiles-model.ts';

test('a connection name never changes its driver category, and Doris stays a database',()=>{
 assert.equal(categoryFor('ssh'),'主机与终端');assert.equal(categoryFor('doris'),'数据库');
 assert.equal(categoryFor('kubernetes'),'Kubernetes');assert.equal(categoryFor('oracle'),undefined);
});
test('legacy import emits public typed metadata, rejects secret-bearing and unknown driver entries',()=>{
 const storage={getItem:(key:string)=>JSON.stringify(key.includes('.db.')?[
  {name:'mysql',host:'db.test',port:3306,username:'reader',database:'demo',kind:'mysql',tls:'verifyIdentity'},
  {name:'doris',host:'doris.test',port:9030,username:'reader',database:'',kind:'doris',tls:'required'},
  {name:'bad',host:'db.test',port:3306,username:'reader',database:'',kind:'mysql',tls:'disabled',password:'synthetic-sensitive-value'},
 ]:[{id:'profile',kind:'ssh',name:'NFS服务',host:'host.test',port:22,username:'ops'}])};
 const result=legacyProfiles(storage);assert.equal(result.profiles.length,3);assert.equal(result.invalidCount,1);
 assert.deepEqual(result.profiles.map(p=>p.settings.driver),['ssh','mysql','doris']);
 assert.ok(!JSON.stringify(result).includes('synthetic-sensitive-value'));
 assert.equal(result.profiles[0].name,'NFS服务');
});
test('saved credential status follows authentication target, not just the display name',()=>{
 const before={driver:'mysql',host:'db.test',port:3306,username:'reader',database:'a',tls:'verifyIdentity'};
 assert.equal(credentialTargetUnchanged(before,{...before,database:'b'}),true);
 assert.equal(credentialTargetUnchanged(before,{...before,username:'other'}),false);
 assert.equal(credentialTargetUnchanged(before,{...before,host:'other.test'}),false);
 assert.equal(credentialTargetUnchanged(before,{...before,tls:'disabled'}),false);
 assert.equal(credentialTargetUnchanged(before,{...before,driver:'doris'}),false);
});

test('legacy display names outside the backend byte limit are isolated without changing original storage',()=>{
 const entry={id:'old',kind:'ssh',host:'host.test',port:22,username:'ops'};
 const raw=JSON.stringify([{...entry,name:'x'.repeat(81)},{...entry,id:'cn',name:'中'.repeat(27)},{...entry,id:'valid',name:'valid'}]);
 const storage={getItem:(key:string)=>key==='aic-ops.connections.v1'?raw:null};const result=legacyProfiles(storage);
 assert.equal(result.profiles.length,1);assert.equal(result.profiles[0].name,'valid');assert.equal(result.invalidCount,2);assert.equal(storage.getItem('aic-ops.connections.v1'),raw);
});

test('MinIO stays in object storage and saved keys are bound to endpoint and signing region',()=>{
 const before={driver:'minio',endpoint:'https://objects.example.test:9000',region:'us-east-1',bucket:''};
 assert.equal(categoryFor('minio'),'对象存储');
 assert.equal(credentialTargetUnchanged(before,{...before,bucket:'photos'}),true);
 assert.equal(credentialTargetUnchanged(before,{...before,endpoint:'https://other.example.test:9000'}),false);
 assert.equal(credentialTargetUnchanged(before,{...before,region:'eu-west-1'}),false);
});
