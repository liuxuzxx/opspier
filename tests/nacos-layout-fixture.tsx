// Only the external IPC boundary is replaced; the real Nacos panel and styles render.
import React from 'react';
import {createRoot} from 'react-dom/client';
import {ConfigProvider,theme} from 'antd';
import {mockIPC} from '@tauri-apps/api/mocks';
import NacosPanel from '../src/NacosPanel';
import '../src/style.css';
const configs=[
 {key:{namespaceId:'',group:'CPaaS_PROD_GROUP',dataId:'cpaas-biz-server-production.yaml'},type:'yaml',md5:'fixture-1',content:'server:\n  port: 8080'},
 {key:{namespaceId:'',group:'MOS_DEV_GROUP',dataId:'mtr-development.yaml'},type:'yaml',md5:'fixture-2',content:'server:\n  port: 8081'},
 {key:{namespaceId:'',group:'CPaaS_DEV_GROUP',dataId:'cpaas-biz-server-development.yaml'},type:'yaml',md5:'fixture-3',content:'server:\n  port: 8082'}
];
const state={calls:[] as {command:string;args:any}[]};
(window as any).__nacosFixture=state;
const match=(value:string,pattern:string)=>!pattern||new RegExp('^'+pattern.split('*').map(s=>s.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')).join('.*')+'$').test(value);
mockIPC((command,args:any)=>{
 state.calls.push({command,args});
 if(command==='nacos_namespaces')return [{id:'',name:'public'},{id:'dev-namespace',name:'开发环境'}];
 if(command==='nacos_configs'){
  const rows=configs.filter(row=>match(row.key.group,args.group)&&match(row.key.dataId,args.dataId));
  return {items:rows,total:rows.length,page:args.page,pageSize:20};
 }
 if(command==='nacos_config')return configs.find(row=>row.key.dataId===args.key.dataId);
 if(command==='nacos_services')return {items:[],total:0,page:1,pageSize:20};
 if(command==='nacos_publish')throw Error('隔离测试：发布失败');
 if(command==='nacos_close')return;
 throw Error('Unexpected test command: '+command);
});
createRoot(document.getElementById('root')!).render(<ConfigProvider theme={{algorithm:theme.darkAlgorithm,token:{colorPrimary:'#5ba8f5',colorBgLayout:'#0c1119',colorBgContainer:'#151d29',borderRadius:7,fontSize:14}}}><div style={{height:'100vh',display:'flex'}}><aside style={{width:300,flexShrink:0,background:'#101823'}}>连接（隔离布局测试）</aside><main style={{padding:12,flex:1,minWidth:0,overflow:'auto'}}><NacosPanel session={{id:'layout-fixture',endpoint:'http://nacos-layout.invalid:8848/nacos',version:'2.4.3',namespace:''}}/></main></div></ConfigProvider>);
