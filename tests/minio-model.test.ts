import test from 'node:test';
import assert from 'node:assert/strict';
import {minioScopeKey,parentPrefix,uploadFileName,formatObjectSize,initialMinioValues} from '../src/minio-model.ts';

test('directory navigation preserves full object prefixes without interpreting object keys as local paths',()=>{
 assert.equal(parentPrefix('photos/2026/'),'photos/');assert.equal(parentPrefix('photos/'),'');assert.equal(parentPrefix(''),'');
 assert.equal(uploadFileName('/home/fixture/中文 report.txt'),'中文 report.txt');assert.equal(uploadFileName('/chosen/a\\b.txt'),'a\\b.txt');assert.equal(uploadFileName('C:\\files\\report.txt'),'report.txt');
 assert.throws(()=>uploadFileName('/home/folder/'));assert.throws(()=>uploadFileName('/home/folder/..'));
});
test('object selection and pagination distinguish bucket, prefix and cursor',()=>{
 assert.notEqual(minioScopeKey('a','b','c'),minioScopeKey('a','bc',''));assert.notEqual(minioScopeKey('a','','x'),minioScopeKey('a','','y'));
 assert.equal(formatObjectSize(0),'0 B');assert.equal(formatObjectSize(1024),'1.0 KiB');
});
test('new MinIO connection defaults are usable without exposing saved keys',()=>{
 const fresh=initialMinioValues(null);assert.equal(fresh.endpoint,'http://localhost:9000');assert.equal(fresh.region,'us-east-1');assert.equal(fresh.bucket,'');
 const saved=initialMinioValues({name:'objects',settings:{driver:'minio',endpoint:'https://objects.example.test',region:'eu-west-1',bucket:'photos'}});
 assert.equal(saved.endpoint,'https://objects.example.test');assert.equal(saved.region,'eu-west-1');assert.equal(saved.bucket,'photos');assert.equal(saved.accessKey,'');assert.equal(saved.secretKey,'');
});
