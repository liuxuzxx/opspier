import {useEffect,useState} from 'react';
import {Button,Layout,Tag,message} from 'antd';
import {BorderOutlined,CloseOutlined,CopyOutlined,MenuFoldOutlined,MenuUnfoldOutlined,MinusOutlined} from '@ant-design/icons';
import {isTauri} from '@tauri-apps/api/core';
import {getCurrentWindow} from '@tauri-apps/api/window';
import {APP_NAME} from './app-info';

type Props={sidebarCollapsed:boolean;onToggleSidebar:()=>void};
type WindowAction='minimize'|'toggleMaximize'|'close';

/** 将工作台工具和原生窗口操作合并，避免额外占用一层系统标题栏。 */
export default function WindowTitlebar({sidebarCollapsed,onToggleSidebar}:Props){
 const [maximized,setMaximized]=useState(false);
 const native=isTauri();
 useEffect(()=>{
  if(!native)return;
  const current=getCurrentWindow();let disposed=false;let unlisten:(()=>void)|undefined;
  const update=async()=>{
   try{const value=await current.isMaximized();if(!disposed)setMaximized(value);}
   catch{if(!disposed)void message.error('读取窗口状态失败');}
  };
  void update();
  void current.onResized(()=>{void update();}).then(stop=>{if(disposed)stop();else unlisten=stop;}).catch(()=>{if(!disposed)void message.error('监听窗口状态失败');});
  return ()=>{disposed=true;unlisten?.();};
 },[native]);
 const run=async(action:WindowAction)=>{
  if(!native)return;
  try{await getCurrentWindow()[action]();}
  catch{void message.error('窗口操作失败，请重试');}
 };
 return <Layout.Header className="app-header" data-tauri-drag-region>
  <div className="brand" data-tauri-drag-region>
   <img src="/ops.svg" alt="" width={18} height={18} draggable={false} data-tauri-drag-region/>
   <strong data-tauri-drag-region>{APP_NAME}</strong>
   <span className="workbench-title" data-tauri-drag-region>运维工作台</span>
  </div>
  <Button size="small" type="text" className="sidebar-toggle" icon={sidebarCollapsed?<MenuUnfoldOutlined/>:<MenuFoldOutlined/>} aria-controls="connection-sidebar" aria-expanded={!sidebarCollapsed} onClick={onToggleSidebar}>{sidebarCollapsed?'展开连接':'收起连接'}</Button>
  <div className="titlebar-drag-region" data-testid="titlebar-drag-region" data-tauri-drag-region/>
  <Tag color="blue" className="titlebar-readonly">数据库与资源浏览只读</Tag>
  {native&&<div className="window-controls" role="group" aria-label="窗口操作">
   <button type="button" className="window-control" aria-label="最小化窗口" title="最小化" onClick={()=>{void run('minimize');}}><MinusOutlined/></button>
   <button type="button" className="window-control" aria-label={maximized?'还原窗口':'最大化窗口'} title={maximized?'还原':'最大化'} onClick={()=>{void run('toggleMaximize');}}>{maximized?<CopyOutlined/>:<BorderOutlined/>}</button>
   <button type="button" className="window-control window-close" aria-label="关闭窗口" title="关闭" onClick={()=>{void run('close');}}><CloseOutlined/></button>
  </div>}
 </Layout.Header>;
}
