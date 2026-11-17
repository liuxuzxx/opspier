import {invoke} from '@tauri-apps/api/core';
export type HttpToolRequest={method:string;url:string;headers:Record<string,string>;body?:string|null};
export type HttpToolResponse={url:string;status:number;statusText:string;headers:Record<string,string>;body:string;truncated:boolean;elapsedMs:number};
export function parseHeaderText(text:string){const result:Record<string,string>={};text.split(/\r?\n/).forEach((line,index)=>{if(!line.trim())return;const split=line.indexOf(':');if(split<=0)throw new Error(`Header 第 ${index+1} 行格式不正确，应为“名称: 值”。`);result[line.slice(0,split).trim()]=line.slice(split+1).trim();});return result;}
export function formatHttpResponse(value:HttpToolResponse){const headers=Object.entries(value.headers).map(([key,item])=>`${key}: ${item}`).join('\r\n');return [`URL: ${value.url}\r\nStatus: ${value.status}${value.statusText?` ${value.statusText}`:''}`,headers,value.body].join('\r\n\r\n')+(value.truncated?'\r\n\r\n[响应正文已截断]':'');}
export function sendHttpTool(request:HttpToolRequest){return invoke<HttpToolResponse>('http_tool_request',{request});}
