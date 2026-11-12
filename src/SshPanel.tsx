import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Alert, Badge, Button, Select, Space, Tag, Typography } from 'antd';
import { invoke } from '@tauri-apps/api/core';
import { readText, writeText } from '@tauri-apps/plugin-clipboard-manager';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { connectionState, formatEndpoint } from './model';
export type SshSession = { id: string; name: string; endpoint: {host:string;port:number;username:string}; status: string };
type Result = { data: number[]; status: string; queuedBytes: number };
function TerminalPane({session, visible, controls, onStatus, onClose}:{session:SshSession;visible:boolean;controls:ReactNode;onStatus:(id:string,status:string)=>void;onClose:()=>void}) {
 const element=useRef<HTMLDivElement>(null); const terminal=useRef<Terminal|null>(null); const active=useRef(visible);active.current=visible;
 const [localError,setLocalError]=useState('');
 const state=connectionState(session.status);
 useEffect(()=>{
  const term=new Terminal({cursorBlink:true,scrollback:5000,fontSize:14,fontFamily:'"DejaVu Sans Mono", "Noto Sans Mono CJK SC", monospace',theme:{background:'#111720',foreground:'#d7e1ee',cursor:'#63b8ff'},allowProposedApi:false});
  const fit=new FitAddon();term.loadAddon(fit);terminal.current=term;term.open(element.current!);
  let gone=false;let timer:ReturnType<typeof setTimeout>|undefined;let inflight=false;let lastStatus='';
  const encoder=new TextEncoder();
  const send=(text:string)=>{const data=encoder.encode(text);if(data.length>64*1024){setLocalError('单次粘贴超过64KiB上限');return;}void invoke('ssh_input',{id:session.id,data:Array.from(data)}).catch(()=>setLocalError('输入队列繁忙或连接已断开'));};
  const input=term.onData(send);
  const resize=()=>{if(!active.current||!element.current?.clientWidth||!element.current?.clientHeight)return;fit.fit();void invoke('ssh_resize',{id:session.id,cols:term.cols,rows:term.rows}).catch(()=>{});};
  let resizeFrame=0;const observer=new ResizeObserver(()=>{cancelAnimationFrame(resizeFrame);resizeFrame=requestAnimationFrame(resize);});observer.observe(element.current!);resize();
  term.attachCustomKeyEventHandler(event=>{
   if(event.ctrlKey&&event.shiftKey&&(event.code==='KeyC'||event.code==='KeyV')){
    if(event.type==='keydown')void (event.code==='KeyC'?writeText(term.getSelection()):readText().then(text=>{if(text)term.paste(text);})).catch(()=>setLocalError('剪贴板访问失败'));
    event.preventDefault();return false;
   }return true;
  });
  const schedule=(delay:number)=>{if(!gone)timer=setTimeout(poll,delay);};
  const poll=async()=>{
   if(gone||inflight)return;inflight=true;
   try {
    const result=await invoke<Result>('ssh_poll',{id:session.id});if(gone)return;
    if(result.status!==lastStatus){lastStatus=result.status;onStatus(session.id,result.status);}
    if(result.data.length){term.write(new Uint8Array(result.data),()=>{inflight=false;if(!gone)schedule(result.queuedBytes?0:24);});}
    else {inflight=false;if(result.status==='connected')schedule(50);}
   }catch{inflight=false;if(!gone){onStatus(session.id,'disconnected');setLocalError('会话已关闭');}}
  };schedule(0);
  return()=>{gone=true;if(timer)clearTimeout(timer);cancelAnimationFrame(resizeFrame);observer.disconnect();input.dispose();term.dispose();terminal.current=null;};
 },[session.id]);
 return <div className="terminal-pane" style={{display:visible?'flex':'none'}} data-testid="ssh-pane">
  <div className="terminal-toolbar"><Space className="terminal-summary" size={8}>{controls}<Badge status={session.status==='connected'?'success':session.status==='connecting'?'processing':session.status==='disconnected'?'default':'error'}/><Tag color={state.color} data-testid="ssh-status">{state.label}</Tag><Typography.Text className="terminal-endpoint" data-testid="ssh-endpoint" title={formatEndpoint(session.endpoint.username,session.endpoint.host,session.endpoint.port)}>{formatEndpoint(session.endpoint.username,session.endpoint.host,session.endpoint.port)}</Typography.Text></Space><Space size={4}>
   <Button size="small" onClick={()=>{const text=terminal.current?.getSelection();if(text)void writeText(text).catch(()=>setLocalError('复制失败'));}}>复制</Button>
   <Button size="small" disabled={session.status!=='connected'} onClick={()=>void readText().then(text=>{if(text)terminal.current?.paste(text);}).catch(()=>setLocalError('粘贴失败'))}>粘贴</Button>
   <Button size="small" onClick={()=>terminal.current?.clear()}>清屏</Button><Button size="small" danger onClick={onClose}>关闭</Button>
  </Space></div>{localError&&<Alert type="warning" message={localError} closable onClose={()=>setLocalError('')}/>}
  <div className="terminal-host" ref={element}/>
 </div>;
}
export default function SshPanel({sessions,selected,onClose}:{sessions:SshSession[];selected:string;onClose:(id:string)=>void}){
 const [layout,setLayout]=useState<'tabs'|'split'>('tabs');
 const [statuses,setStatuses]=useState<Record<string,string>>({});
 const status=(id:string,value:string)=>setStatuses(old=>({...old,[id]:value}));
 const visibleIds=layout==='split'?sessions.slice(-4).map(v=>v.id):[selected||sessions[0]?.id];
 const controls=<Select aria-label="终端视图" size="small" value={layout} onChange={setLayout} options={[{value:'tabs',label:'单窗格'},{value:'split',label:'四分屏'}]} style={{width:104}}/>;
 return <main className="module-content"><div className={`terminal-grid ${layout==='split'?'split':''}`}>{sessions.map(session=><TerminalPane key={session.id} session={{...session,status:statuses[session.id]??session.status}} visible={visibleIds.includes(session.id)} controls={session.id===visibleIds[0]?controls:null} onStatus={status} onClose={()=>onClose(session.id)}/>)}</div></main>;

}
