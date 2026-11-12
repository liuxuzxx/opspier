import type {MinioSettings} from './profiles-model';
export type MinioSession={id:string;endpoint:string;bucket:string};
export type MinioObject={key:string;size:number;directory:boolean;modified:string|null};
export type MinioPage={objects:MinioObject[];nextCursor:string|null};
export type MinioTransfer={id:string;bucket:string;key:string;direction:'upload'|'download';status:'running'|'completed'|'cancelled'|'failed';bytes:number;total:number;error:string|null};
export function minioScopeKey(bucket:string,prefix:string,cursor:string){return JSON.stringify([bucket,prefix,cursor]);}
export function parentPrefix(prefix:string){return prefix.replace(/\/$/,'').replace(/[^/]*$/,'');}
export function uploadFileName(path:string){const name=(path.startsWith('/')?path.split('/'):path.split(/[\\/]/)).pop()??'';if(!name||name==='.'||name==='..'||/[\u0000-\u001f\u007f]/u.test(name))throw new Error('请拖入或选择普通文件');return name;}
export function formatObjectSize(size:number){if(size<1024)return `${size} B`;const units=['KiB','MiB','GiB','TiB'];let value=size/1024;let unit=0;while(value>=1024&&unit<units.length-1){value/=1024;unit++;}return `${value.toFixed(1)} ${units[unit]}`;}
export function initialMinioValues(profile:{name:string;settings:MinioSettings}|null){return {name:profile?.name??'',endpoint:profile?.settings.endpoint??'http://localhost:9000',region:profile?.settings.region??'us-east-1',bucket:profile?.settings.bucket??'',accessKey:'',secretKey:''};}
