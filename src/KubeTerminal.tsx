import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {open as chooseFile} from '@tauri-apps/plugin-dialog';
import {readText,writeText} from '@tauri-apps/plugin-clipboard-manager';
import {Alert,Button,Input,Space,Tag,Typography} from 'antd';
import {Terminal} from '@xterm/xterm';import {FitAddon} from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import {KubeTerminalController} from './kube-terminal';
import type {KubeSelection} from './kube-model';
export default function KubeTerminal({selection,namespace,visible,suspended}:{selection:KubeSelection;namespace:string;visible:boolean;suspended:boolean}){
 const [,render]=useState(0);const [controller]=useState(()=>new KubeTerminalController(selection,namespace,invoke,()=>render(old=>old+1)));
 const state=controller.state;const host=useRef<HTMLDivElement>(null);const terminal=useRef<Terminal|null>(null);const fit=useRef<FitAddon|null>(null);const active=useRef(visible);active.current=visible;
 const [kubectlPath,setKubectlPath]=useState('');const [shellPath,setShellPath]=useState('');const [localError,setLocalError]=useState('');
 useEffect(()=>{controller.setSuspended(suspended);},[controller,suspended]);
 useEffect(()=>{void controller.namespace(namespace);},[controller,namespace]);
 useEffect(()=>{
  const term=new Terminal({cursorBlink:true,scrollback:5000,fontSize:14,fontFamily:'"DejaVu Sans Mono", "Noto Sans Mono CJK SC", monospace',theme:{background:'#111720',foreground:'#d7e1ee'},allowProposedApi:false});const addon=new FitAddon();term.loadAddon(addon);term.open(host.current!);terminal.current=term;fit.current=addon;
  const input=term.onData(text=>{if(!controller.send(text))setLocalError('输入尚未接受：终端未就绪、正在切换目标或输入队列已满。');});
  const resize=()=>{if(!active.current||!host.current?.clientWidth||!host.current.clientHeight)return;addon.fit();void controller.resize(term.cols,term.rows).catch(()=>{});};let frame=0;
  const observer=new ResizeObserver(()=>{cancelAnimationFrame(frame);frame=requestAnimationFrame(resize);});observer.observe(host.current!);
  term.attachCustomKeyEventHandler(event=>{if(event.ctrlKey&&event.shiftKey&&(event.code==='KeyC'||event.code==='KeyV')){if(event.type==='keydown')void(event.code==='KeyC'?writeText(term.getSelection()):readText().then(text=>{if(text&&controller.canInput)term.paste(text);})).catch(()=>setLocalError('剪贴板访问失败'));event.preventDefault();return false;}return true;});
  return()=>{observer.disconnect();cancelAnimationFrame(frame);input.dispose();controller.dispose();term.dispose();terminal.current=null;fit.current=null;};
 },[controller]);
 useEffect(()=>{if(!visible)return;requestAnimationFrame(()=>{if(host.current?.clientWidth){fit.current?.fit();const term=terminal.current;if(term){void controller.resize(term.cols,term.rows).catch(()=>{});term.focus();}}});},[visible,state.status,controller]);
 useEffect(()=>{
  if(state.status!=='connected')return;let disposed=false;let timer:ReturnType<typeof setTimeout>|undefined;
  const schedule=(delay:number)=>{if(!disposed)timer=setTimeout(poll,delay);};
  const poll=async()=>{try{const result=await controller.poll();if(disposed||!result)return;if(result.data.length){terminal.current?.write(new Uint8Array(result.data),()=>{if(!result.done)schedule(result.queuedBytes?0:24);});}else if(!result.done)schedule(50);}catch(error){if(!disposed)setLocalError(String(error));}};
  schedule(0);return()=>{disposed=true;if(timer)clearTimeout(timer);};
 },[controller,state.status]);
 async function selectPath(kind:'kubectl'|'shell'){try{const selected=await chooseFile({multiple:false,directory:false,title:kind==='kubectl'?'选择本机 kubectl 可执行文件':'选择本机 bash / cmd.exe'});if(typeof selected==='string'){if(kind==='kubectl')setKubectlPath(selected);else setShellPath(selected);}}catch{setLocalError('无法选择本机可执行文件');}}
 return <div className="kube-terminal" style={{display:visible?'block':'none'}} data-testid="kube-terminal">
  <Space wrap size={8} style={{marginBottom:8}}><Tag color={state.status==='connected'?'green':'default'} data-testid="kube-terminal-status">{{closed:'未打开',opening:'正在启动',connected:'运行中',exited:'已退出',error:'启动失败'}[state.status]}</Tag><Typography.Text>默认目标：{selection.context} / {state.status==='connected'?(state.appliedNamespace||'default'):(namespace||'default')}</Typography.Text><Typography.Text type="secondary">{state.updating?`正在更新私有配置至 ${namespace||'default'}…`:'终端命令按当前凭据的 RBAC 执行；显式参数可覆盖默认目标。'}</Typography.Text></Space>
  <Space wrap size={6} style={{marginBottom:8}}>
   <Input aria-label="kubectl可执行文件" placeholder="kubectl 路径（空为 PATH）" value={kubectlPath} onChange={e=>setKubectlPath(e.target.value)} disabled={state.status==='opening'||state.status==='connected'} style={{width:260}}/><Button size="small" onClick={()=>void selectPath('kubectl')} disabled={state.status==='opening'||state.status==='connected'}>选择 kubectl</Button>
   <Input aria-label="本地shell可执行文件" placeholder="shell 路径（Linux bash / Windows cmd）" value={shellPath} onChange={e=>setShellPath(e.target.value)} disabled={state.status==='opening'||state.status==='connected'} style={{width:290}}/><Button size="small" onClick={()=>void selectPath('shell')} disabled={state.status==='opening'||state.status==='connected'}>选择 shell</Button>
   <Button type="primary" size="small" data-testid="kube-terminal-open" disabled={suspended||state.status==='opening'||state.status==='connected'} onClick={()=>{setLocalError('');terminal.current?.clear();void controller.open(kubectlPath,shellPath);}}>打开终端</Button>
   <Button size="small" disabled={!state.scope} onClick={()=>void controller.close()}>关闭终端</Button>
   <Button size="small" onClick={()=>{const text=terminal.current?.getSelection();if(text)void writeText(text).catch(()=>setLocalError('复制失败'));}}>复制</Button>
   <Button size="small" disabled={!controller.canInput} onClick={()=>void readText().then(text=>{if(text)terminal.current?.paste(text);}).catch(()=>setLocalError('粘贴失败'))}>粘贴</Button><Button size="small" onClick={()=>terminal.current?.clear()}>清屏</Button>
  </Space>
  {(state.error||localError)&&<Alert type="warning" showIcon title={state.error||localError} closable onClose={()=>setLocalError('')} style={{marginBottom:8}}/>}
  {state.namespace!==state.appliedNamespace&&state.status==='connected'&&!state.updating&&<Button size="small" onClick={()=>void controller.namespace(namespace)}>重试更新默认命名空间</Button>}
  <div ref={host} className="kube-terminal-host" aria-label="kubectl交互终端"/>
  <Typography.Text type="secondary">Ctrl+C 中断前台命令；Ctrl+Shift+C / V 复制粘贴。终端保留内存滚动区与会话内历史，关闭后清除私有 kubeconfig。</Typography.Text>
 </div>;
}
