import {useEffect} from 'react';
import {ConfigProvider,Layout,Tag,Typography,theme} from 'antd';
import ConnectionManager from './ConnectionManager';
import {APP_NAME,APP_VERSION} from './app-info';
export default function App(){
 useEffect(()=>{document.title=`${APP_NAME} ${APP_VERSION}`;},[]);
 return <ConfigProvider theme={{algorithm:theme.darkAlgorithm,token:{colorPrimary:'#5ba8f5',colorBgLayout:'#0c1119',colorBgContainer:'#151d29',borderRadius:7,fontSize:14}}}><Layout className="app-shell"><Layout.Header className="app-header"><div className="brand"><img src="/ops.svg" alt="" width={34} height={34}/><strong>{APP_NAME}</strong></div><Typography.Text type="secondary">运维工作台</Typography.Text><Tag color="blue" style={{marginLeft:'auto'}}>数据库与 K8s 只读</Tag></Layout.Header><Layout.Content className="app-content"><ConnectionManager/></Layout.Content><Layout.Footer className="app-footer"><Typography.Text type="secondary">{APP_NAME} {APP_VERSION} · SSH 交互 / 数据库查询 / Kubernetes 浏览 · 系统凭据服务</Typography.Text></Layout.Footer></Layout></ConfigProvider>;
}
