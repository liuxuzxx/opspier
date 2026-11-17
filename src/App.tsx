import {useEffect,useState} from 'react';
import {ConfigProvider,Layout,Typography,theme} from 'antd';
import ConnectionManager from './ConnectionManager';
import WindowTitlebar from './WindowTitlebar';
import {APP_NAME,APP_VERSION} from './app-info';
export default function App(){
 const [sidebarCollapsed,setSidebarCollapsed]=useState(false);
 useEffect(()=>{document.title=`${APP_NAME} ${APP_VERSION}`;},[]);
 return <ConfigProvider theme={{algorithm:theme.darkAlgorithm,token:{colorPrimary:'#5ba8f5',colorBgLayout:'#0c1119',colorBgContainer:'#151d29',borderRadius:7,fontSize:14}}}><Layout className="app-shell"><WindowTitlebar sidebarCollapsed={sidebarCollapsed} onToggleSidebar={()=>setSidebarCollapsed(value=>!value)}/><Layout.Content className="app-content"><ConnectionManager sidebarCollapsed={sidebarCollapsed}/></Layout.Content><Layout.Footer className="app-footer"><Typography.Text type="secondary">{APP_NAME} {APP_VERSION} · SSH 交互 / 数据库查询 / Kubernetes 浏览与终端 / Nacos / MinIO · 系统凭据服务</Typography.Text></Layout.Footer></Layout></ConfigProvider>;
}
