export type NacosSettings={driver:'nacos';endpoint:string;authentication:'password'|'none';namespace:string};
export type NacosSession={id:string;endpoint:string;version:string;namespace:string};
export type NacosNamespace={id:string;name:string};
export type ConfigKey={namespaceId:string;group:string;dataId:string};
export type ConfigItem={key:ConfigKey;type:string;md5:string};
export type ConfigView=ConfigItem&{content:string};
export type ConfigHistory={id:string;modified:string;operation:string};
export type Page<T>={items:T[];total:number;page:number;pageSize:number};
export type ServiceKey={namespaceId:string;group:string;name:string};
export type ServiceItem={key:ServiceKey;instanceCount:number;healthyCount:number;clusterCount:number};
export type InstanceKey={ip:string;port:number;cluster:string;ephemeral:boolean};
export type NacosInstance={key:InstanceKey;healthy:boolean;enabled:boolean;weight:number;metadata:Record<string,string>;snapshot:string};
export type NacosServiceView={key:ServiceKey;protectThreshold:number;metadata:Record<string,string>;selector:unknown;snapshot:string;instances:NacosInstance[]};
export function nacosScopeKey(namespace:string,...parts:string[]){return JSON.stringify([namespace==='public'?'':namespace,...parts]);}
export function parseMetadata(value:string):Record<string,string>{const data:unknown=JSON.parse(value);if(!data||typeof data!=='object'||Array.isArray(data)||Object.values(data).some(v=>typeof v!=='string'))throw new Error('元数据必须为字符串键值的 JSON 对象');if(Object.keys(data).length>256||new TextEncoder().encode(value).length>65536)throw new Error('元数据超过256项或64KiB上限');return data as Record<string,string>;}
export function nacosDefaults(){return {name:'',endpoint:'http://localhost:8848/nacos',authentication:'password' as 'password'|'none',namespace:'',username:'',password:''};}
export function requiresCredential(settings:{driver:string;authentication?:string}){return settings.driver!=='nacos'||settings.authentication!=='none';}
export function configDirty(original:string,draft:string){return original!==draft;}
export function canApplyNacosResult(currentScope:string,resultScope:string,currentGeneration:number,resultGeneration:number){return currentScope===resultScope&&currentGeneration===resultGeneration;}
// Nacos blur queries only match substrings when the pattern contains wildcards.
export function nacosConfigPattern(value:string){const query=value.trim();return !query||query.includes('*')?query:`*${query}*`;}
