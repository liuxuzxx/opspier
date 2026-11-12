export type MinioSettings = {driver:"minio";endpoint:string;region:string;bucket:string};
export type DatabaseSettings = { driver:'mysql'|'doris'; host:string; port:number; username:string; database:string; tls:'auto'|'verifyIdentity'|'required'|'disabled' };
export type SshSettings = { driver:'ssh'; host:string; port:number; username:string; authentication:'password'|'key'; privateKeyPath:string|null };
export type KubeSettings = { driver:'kubernetes'; context:string|null; namespace:string|null; contexts:string[]; snapshotId:string };
export type ProfileSettings = DatabaseSettings|SshSettings|KubeSettings|MinioSettings;
export type Profile = { id:string; name:string; settings:ProfileSettings; revision:number; credentialSaved:boolean };
export type ProfileListing = {profiles:Profile[];cleanupPending:number};
export type MutationReceipt = {durabilityConfirmed:boolean;profile:Profile|null;cleanupPending:number};
export type LegacyProfile = {sourceId:string;name:string;settings:DatabaseSettings|SshSettings};

export function categoryFor(driver:string):string|undefined {
 return ({ssh:'主机与终端',mysql:'数据库',doris:'数据库',kubernetes:'Kubernetes',minio:'对象存储'} as Record<string,string>)[driver];
}
export function credentialTargetUnchanged(before:Record<string,unknown>,after:Record<string,unknown>):boolean {
 const target=(s:Record<string,unknown>):string|undefined=>{
  if(s.driver==='ssh')return JSON.stringify([s.driver,s.host,s.port,s.username,s.authentication,s.privateKeyPath]);
  if(s.driver==='mysql'||s.driver==='doris')return JSON.stringify([s.driver,s.host,s.port,s.username,s.tls]);
  if(s.driver==='minio')return JSON.stringify([s.driver,s.endpoint,s.region]);
  if(s.driver==='kubernetes'&&typeof s.snapshotId==='string')return JSON.stringify([s.driver,s.snapshotId]);
 };
 const current=target(before);return current!==undefined&&current===target(after);
}

export function legacyProfiles(storage:Pick<Storage,'getItem'>):{profiles:LegacyProfile[];invalidCount:number} {
 const profiles:LegacyProfile[]=[];let invalidCount=0;
 const read=(key:string):unknown[]=>{
  try{const raw=storage.getItem(key);if(!raw)return [];const value:unknown=JSON.parse(raw);if(Array.isArray(value))return value;invalidCount++;}
  catch{invalidCount++;}return [];
 };
 const object=(value:unknown,keys:string[]):value is Record<string,unknown>=>!!value&&typeof value==='object'&&!Array.isArray(value)&&Object.keys(value).every(k=>keys.includes(k));
 const text=(value:unknown,max=128):value is string=>typeof value==='string'&&value.trim().length>0&&new TextEncoder().encode(value).length<=max&&!/[\u0000-\u001f\u007f-\u009f]/u.test(value);
 const port=(value:unknown):value is number=>typeof value==='number'&&Number.isInteger(value)&&value>0&&value<=65535;
 for(const value of read('aic-ops.connections.v1')) {
  if(!object(value,['id','kind','name','host','port','username'])||value.kind!=='ssh'||!text(value.name,80)||!text(value.host,253)||!port(value.port)||!text(value.username)||typeof value.id!=='string'||value.id.length>200){invalidCount++;continue;}
  profiles.push({sourceId:`ssh:${value.id}`,name:value.name,settings:{driver:'ssh',host:value.host,port:value.port,username:value.username,authentication:'password',privateKeyPath:null}});
 }
 for(const value of read('aic-ops.db.connections.v1')) {
  if(!object(value,['name','host','port','username','database','kind','tls'])||!text(value.name,80)||!text(value.host,253)||!port(value.port)||!text(value.username)||(typeof value.database!=='string'||new TextEncoder().encode(value.database).length>128||/[\u0000-\u001f\u007f-\u009f]/u.test(value.database))||!['mysql','doris'].includes(String(value.kind))||!['auto','verifyIdentity','required','disabled'].includes(String(value.tls))){invalidCount++;continue;}
  profiles.push({sourceId:'db',name:value.name,settings:{driver:value.kind as 'mysql'|'doris',host:value.host,port:value.port,username:value.username,database:value.database,tls:value.tls as DatabaseSettings['tls']}});
 }
 return {profiles,invalidCount};
}
