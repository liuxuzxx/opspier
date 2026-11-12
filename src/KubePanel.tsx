import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {Alert,AutoComplete,Button,Card,Checkbox,Empty,Input,Select,Space,Table,Tabs,Tag,Tooltip,Typography} from 'antd';
import {RESOURCE_KINDS,acceptKubeResult,appendLog,namespaceColor,statusColor} from './kube-model';
import type {KubeDetail,KubeLogPoll,KubeRow,KubeSelection} from './kube-model';
import {KubeBrowser,browserScopeKey} from './kube-browser';
import type {BrowserScope,BrowserSnapshot,BrowserPage,BrowserState} from './kube-browser';
type Resource={id:string;group:string;version:string;plural:string;kind:string;namespaced:boolean};
type Catalog={sessionId:string;generation:number;catalogRevision:number;resources:Resource[];warning:string|null};
type Desired={resourceId:string;namespace:string;search:string;cursor:string;page:number};
const labels:Record<string,string>={pods:'Pods',deployments:'Deployments',statefulsets:'StatefulSets',daemonsets:'DaemonSets',jobs:'Jobs',cronjobs:'CronJobs',services:'Services',configmaps:'ConfigMaps',secrets:'Secrets（仅元数据）',persistentvolumeclaims:'PVC',persistentvolumes:'PV',events:'Events',customresourcedefinitions:'CRD 定义'};
const fallback:Resource[]=[...RESOURCE_KINDS,'customresourcedefinitions'].map(id=>({id,group:'',version:'v1',plural:id,kind:labels[id]??id,namespaced:!['persistentvolumes','customresourcedefinitions'].includes(id)}));
const mono={fontFamily:'ui-monospace, SFMono-Regular, Menlo, monospace',whiteSpace:'pre-wrap' as const,overflowWrap:'anywhere' as const};
function resourceLabel(resource:Resource){return resource.id.startsWith('custom:')?`${resource.kind} · ${resource.group}/${resource.version} · ${resource.namespaced?'命名空间':'集群'}`:labels[resource.id]??resource.kind;}
export default function KubePanel({initialSelection}:{initialSelection:KubeSelection}){
 const [viewportHeight,setViewportHeight]=useState(()=>window.innerHeight);
 useEffect(()=>{const resize=()=>setViewportHeight(window.innerHeight);window.addEventListener('resize',resize);return()=>window.removeEventListener('resize',resize);},[]);
 const mounted=useRef(true);const [selection,setSelection]=useState(initialSelection);const session=useRef<KubeSelection|null>(initialSelection);
 const [catalog,setCatalog]=useState({catalogRevision:0,resources:fallback,warning:null as string|null});
 const [desired,setDesired]=useState<Desired>({resourceId:'pods',namespace:initialSelection.namespace,search:'',cursor:'',page:0});
 const [namespaceInput,setNamespaceInput]=useState(initialSelection.namespace);const [namespaceChoices,setNamespaceChoices]=useState<string[]>([]);const [namespaceWarning,setNamespaceWarning]=useState('');
 const [error,setError]=useState('');const [pagingNotice,setPagingNotice]=useState('');const [autoRefresh,setAutoRefresh]=useState(true);const [switching,setSwitching]=useState(false);const transitioning=useRef(false);
 const [browserState,setBrowserState]=useState<BrowserState>({snapshot:null,pending:null,interactive:false,error:''});
 const browser=useRef<KubeBrowser|null>(null);
 if(!browser.current)browser.current=new KubeBrowser((scope,id)=>invoke<BrowserPage>('kube_query',{sessionId:scope.sessionId,generation:scope.generation,request:{requestId:id,catalogRevision:scope.catalogRevision,resourceId:scope.resourceId,namespace:scope.namespace,search:scope.search,continueToken:scope.cursor}}),(scope,id)=>invoke('kube_cancel_query',{sessionId:scope.sessionId,generation:scope.generation,requestId:id}),value=>{if(mounted.current)setBrowserState(value);});
 const controller=browser.current;
 const resource=catalog.resources.find(item=>item.id===desired.resourceId)??fallback[0];
 const wanted:BrowserScope={sessionId:selection.sessionId,generation:selection.generation,context:selection.context,catalogRevision:catalog.catalogRevision,resourceId:desired.resourceId,namespace:resource.namespaced?desired.namespace:'',search:desired.search.trim(),cursor:desired.cursor,page:desired.page};
 const wantedRef=useRef(wanted);wantedRef.current=wanted;const wantedKey=browserScopeKey(wanted);
 const snapshot=browserState.snapshot;const localPage=snapshot?.localPage??0;const interactive=browserState.interactive&&!switching&&!!snapshot&&browserScopeKey(snapshot.scope)===wantedKey;
 const cursors=useRef<string[]>(['']);
 const [selected,setSelected]=useState<KubeRow|null>(null);const selectedSnapshot=useRef<BrowserSnapshot|null>(null);const [detail,setDetail]=useState<KubeDetail|null>(null);const detailEpoch=useRef(0);
 const [container,setContainer]=useState<string>();const [follow,setFollow]=useState(true);const [logs,setLogs]=useState('');const [logSearch,setLogSearch]=useState('');const [dropped,setDropped]=useState(0);const [logId,setLogId]=useState<string|null>(null);const logRef=useRef<string|null>(null);const [logStatus,setLogStatus]=useState('');const [startingLogs,setStartingLogs]=useState(false);const logEpoch=useRef(0);const visibleLog=useRef('');
 function clearDetails(){
  detailEpoch.current++;logEpoch.current++;selectedSnapshot.current=null;setSelected(null);setDetail(null);setContainer(undefined);setStartingLogs(false);visibleLog.current='';setLogs('');setDropped(0);setLogStatus('');
  const old=logRef.current;logRef.current=null;setLogId(null);const value=session.current;if(old&&value)void invoke('kube_logs_stop',{sessionId:value.sessionId,generation:value.generation,logId:old}).catch(()=>{});
 }
 useEffect(()=>{if(switching)return;clearDetails();void controller.run(wanted);},[wantedKey,switching]);
 useEffect(()=>{
  if(!autoRefresh||browserState.pending||!snapshot||!interactive)return;
  const timer=setTimeout(()=>void controller.run(wantedRef.current,true),5000);return()=>clearTimeout(timer);
 },[autoRefresh,browserState.pending,snapshot,interactive]);
 useEffect(()=>{
  if(!browserState.pending&&browserState.error.includes('410')&&desired.cursor){setPagingNotice('服务器分页已过期，已回到首页重新加载。');cursors.current=[''];setDesired(old=>({...old,cursor:'',page:0}));}
 },[browserState.pending,browserState.error,desired.cursor]);
 useEffect(()=>{
  let disposed=false;const value=selection;
  void invoke<{sessionId:string;generation:number;namespaces:string[];truncated:boolean}>('kube_namespaces',{sessionId:value.sessionId,generation:value.generation}).then(result=>{
   if(disposed||!acceptKubeResult(session.current,result))return;setNamespaceChoices([...new Set([value.namespace,...result.namespaces])].filter(Boolean));setNamespaceWarning(result.truncated?'命名空间列表已限量，可手动输入完整名称。':'');
  }).catch(e=>{if(!disposed&&acceptKubeResult(session.current,value))setNamespaceWarning(`命名空间列表读取失败，可手动输入：${String(e)}`);});
  void invoke<Catalog>('kube_resources',{sessionId:value.sessionId,generation:value.generation}).then(result=>{
   if(disposed||!acceptKubeResult(session.current,result))return;setCatalog({catalogRevision:result.catalogRevision,resources:result.resources,warning:result.warning});
   setDesired(old=>result.resources.some(item=>item.id===old.resourceId)?old:{...old,resourceId:'pods',cursor:'',page:0});
  }).catch(e=>{if(!disposed&&acceptKubeResult(session.current,value))setCatalog(old=>({...old,warning:`CRD发现失败，内置资源仍可用：${String(e)}`}));});
  return()=>{disposed=true;};
 },[selection.sessionId,selection.generation]);
 useEffect(()=>()=>{mounted.current=false;controller.dispose();detailEpoch.current++;logEpoch.current++;const value=session.current;session.current=null;if(value)void invoke('kube_close',{sessionId:value.sessionId,generation:value.generation}).catch(()=>{});},[]);
 function changeNamespace(value:string){setNamespaceInput(value);cursors.current=[''];setPagingNotice('');setDesired(old=>({...old,namespace:value.trim(),cursor:'',page:0}));}
 function changeSearch(value:string){cursors.current=[''];setPagingNotice('');setDesired(old=>({...old,search:value,cursor:'',page:0}));}
 async function switchSelection(context:string){
  const previous=session.current;if(!previous||transitioning.current)return;transitioning.current=true;setSwitching(true);controller.invalidate();clearDetails();setError('');
  try{
   const result=await invoke<KubeSelection>('kube_select',{sessionId:previous.sessionId,generation:previous.generation,context,namespace:null});
   if(!mounted.current||session.current?.sessionId!==previous.sessionId||result.sessionId!==previous.sessionId||result.generation!==previous.generation+1)return;
   session.current=result;setSelection(result);setCatalog({catalogRevision:0,resources:fallback,warning:null});setNamespaceInput(result.namespace);setNamespaceChoices([]);cursors.current=[''];setDesired(old=>({...old,resourceId:RESOURCE_KINDS.includes(old.resourceId as typeof RESOURCE_KINDS[number])?old.resourceId:'pods',namespace:result.namespace,cursor:'',page:0}));
  }catch(e){if(mounted.current)setError(String(e));}
  finally{transitioning.current=false;if(mounted.current)setSwitching(false);}
 }
 function identityArgs(value:BrowserSnapshot,row:KubeRow){return {sessionId:value.scope.sessionId,generation:value.scope.generation,catalogRevision:value.data.catalogRevision,scopeRevision:value.data.scopeRevision,resourceId:value.scope.resourceId,namespace:row.namespace};}
 async function view(row:KubeRow){
  const shown=controller.state.snapshot;if(!shown||!controller.state.interactive||browserScopeKey(shown.scope)!==browserScopeKey(wantedRef.current))return;
  clearDetails();const request=++detailEpoch.current;selectedSnapshot.current=shown;setSelected(row);setContainer(row.containers[0]);setError('');
  try{const result=await invoke<KubeDetail>('kube_resource_detail',{...identityArgs(shown,row),name:row.name});if(request===detailEpoch.current&&acceptKubeResult(session.current,result)){setDetail(result);setContainer(result.containers[0]);}}
  catch(e){if(request===detailEpoch.current)setError(String(e));}
 }
 async function startLogs(){
  const shown=selectedSnapshot.current;if(!shown||!selected||!container||shown.scope.resourceId!=='pods'||!controller.state.interactive)return;
  const request=++logEpoch.current;visibleLog.current='';setLogs('');setDropped(0);setLogStatus('连接日志流…');setError('');setStartingLogs(true);
  try{
   const result=await invoke<{sessionId:string;generation:number;logId:string}>('kube_resource_logs_start',{...identityArgs(shown,selected),pod:selected.name,container,follow,tailLines:200});
   if(request===logEpoch.current&&acceptKubeResult(session.current,result)){logRef.current=result.logId;setLogId(result.logId);setLogStatus(follow?'正在跟随':'正在读取');}
   else if(result.sessionId===shown.scope.sessionId)void invoke('kube_logs_stop',{sessionId:result.sessionId,generation:result.generation,logId:result.logId}).catch(()=>{});
  }catch(e){if(request===logEpoch.current){setError(String(e));setLogStatus('读取失败');}}
  finally{if(request===logEpoch.current)setStartingLogs(false);}
 }
 useEffect(()=>{
  if(!logId)return;const value=session.current;if(!value)return;const expected=logEpoch.current;let disposed=false;let running=false;
  const poll=async()=>{if(disposed||running)return;running=true;try{
   const result=await invoke<KubeLogPoll>('kube_logs_poll',{sessionId:value.sessionId,generation:value.generation,logId});if(disposed||expected!==logEpoch.current||!acceptKubeResult(session.current,result)||result.logId!==logId)return;
   const next=appendLog(visibleLog.current,result.text);visibleLog.current=next.text;setLogs(next.text);setDropped(old=>old+result.droppedBytes+next.dropped);
   if(result.done){logRef.current=null;setLogId(null);setLogStatus(result.error??'日志流已结束');}
  }catch(e){if(!disposed&&expected===logEpoch.current){logRef.current=null;setLogId(null);setError(String(e));setLogStatus('日志流已停止');}}finally{running=false;}};
  void poll();const timer=setInterval(()=>void poll(),300);return()=>{disposed=true;clearInterval(timer);};
 },[logId,selection]);
 async function stopLogs(){logEpoch.current++;const old=logRef.current;logRef.current=null;setLogId(null);setStartingLogs(false);setLogStatus('已停止');const value=session.current;if(old&&value)try{await invoke('kube_logs_stop',{sessionId:value.sessionId,generation:value.generation,logId:old});}catch(e){setError(String(e));}}
 function changePage(target:number){
  const shown=controller.state.snapshot;if(!shown||!interactive||target<0)return;
  if(shown.scope.search){controller.setLocalPage(target);return;}
  const cursor=target>shown.scope.page?shown.data.continueToken:cursors.current[target];if(target>shown.scope.page&&!cursor)return;
  if(target>shown.scope.page)cursors.current=[...cursors.current.slice(0,shown.scope.page+1),cursor];setDesired(old=>({...old,cursor:cursor??'',page:target}));
 }
 const shownResource=snapshot?catalog.resources.find(item=>item.id===snapshot.scope.resourceId):undefined;
 const shownPage=snapshot?.scope.search?localPage:snapshot?.scope.page??0;
 const items=snapshot?.scope.search?snapshot.data.items.slice(localPage*100,(localPage+1)*100):snapshot?.data.items??[];
 const hasNext=snapshot?.scope.search?(localPage+1)*100<snapshot.data.items.length:!!snapshot?.data.continueToken;
 const filteredLogs=logSearch?logs.split('\n').filter(line=>line.toLocaleLowerCase().includes(logSearch.toLocaleLowerCase())).join('\n'):logs;
 const selectedResource=selectedSnapshot.current?.scope.resourceId;
 return <Space className="compact-workspace" orientation="vertical" size={8} style={{width:'100%'}}>
  <div className="workspace-toolbar"><Space wrap size={8}>
   <Tooltip title="只读浏览；核心Secrets仅显示安全元数据"><Tag color="blue">只读</Tag></Tooltip>
   <Select aria-label="Context" size="small" value={selection.context} options={selection.contexts.map(value=>({value,label:value}))} onChange={value=>void switchSelection(value)} disabled={switching} style={{width:200}}/>
   <AutoComplete aria-label="命名空间" size="small" allowClear value={namespaceInput} options={namespaceChoices.map(value=>({value,label:<Tag color={namespaceColor(value)}>{value}</Tag>}))} onSelect={changeNamespace} onClear={()=>changeNamespace('')} onChange={value=>{setNamespaceInput(value);if(!value)changeNamespace('');}} filterOption={(input,option)=>input===desired.namespace||String(option?.value??'').toLowerCase().includes(input.toLowerCase())} disabled={switching||!resource.namespaced} style={{width:210}}><Input aria-label="手动输入命名空间" maxLength={63} placeholder="全部命名空间 · 可选可输入" onPressEnter={()=>changeNamespace(namespaceInput)}/></AutoComplete>
   <Typography.Text type="secondary">{resource.namespaced?(desired.namespace||'全部命名空间'):'集群范围'}</Typography.Text>
  </Space></div>
  {namespaceWarning&&<Typography.Text type="warning">{namespaceWarning}</Typography.Text>}
  {catalog.warning&&<Typography.Text type="warning">{catalog.warning}</Typography.Text>}
  {(error||browserState.error)&&<Alert type="error" showIcon title={error||browserState.error}/>}
  {pagingNotice&&<Typography.Text type="warning">{pagingNotice}</Typography.Text>}
  <Card size="small" className="data-card"><Space wrap size={8} style={{marginBottom:8}}>
   <Select aria-label="资源类型" value={desired.resourceId} showSearch optionFilterProp="label" options={catalog.resources.map(item=>({value:item.id,label:resourceLabel(item)}))} onChange={resourceId=>{cursors.current=[''];setDesired(old=>({...old,resourceId,cursor:'',page:0}));}} disabled={switching} style={{width:300}}/>
   <Input.Search aria-label="搜索资源" placeholder="搜索当前类型与命名空间的全部页" maxLength={256} value={desired.search} onChange={e=>changeSearch(e.target.value)} onSearch={changeSearch} disabled={switching} style={{width:300}} allowClear/>
   <Button size="small" onClick={()=>void controller.run(wantedRef.current)} disabled={!!browserState.pending||switching}>刷新</Button>
   <Checkbox checked={autoRefresh} onChange={e=>setAutoRefresh(e.target.checked)}>自动刷新</Checkbox>
   <Typography.Text type="secondary" data-testid="kube-query-status">{browserState.pending?'正在加载，保留上次结果':autoRefresh?'每5秒更新':''}</Typography.Text>
  </Space>
  {snapshot?<><Typography.Paragraph data-testid="kube-data-scope" type="secondary" style={{marginBottom:8}}>当前数据：{snapshot.scope.context} / {shownResource?resourceLabel(shownResource):snapshot.scope.resourceId} / {shownResource?.namespaced===false?'集群范围':snapshot.scope.namespace||'全部命名空间'}{snapshot.scope.search?` · 搜索“${snapshot.scope.search}” · 扫描${snapshot.data.scanned}项/${snapshot.data.pages}页`:''} · 第{shownPage+1}页{!interactive?' · 只读快照，等待当前范围加载':''}</Typography.Paragraph>{snapshot.data.truncated&&<Alert type="warning" title={snapshot.data.reason??'结果已限量，不代表完整全集'} style={{marginBottom:8}}/>}</>:<Empty description={browserState.pending?'正在加载资源…':'暂无资源快照'}/>}
  <Table<KubeRow> rowKey={row=>`${row.namespace}/${row.name}`} size="small" virtual scroll={{x:900,y:selected?Math.max(180,Math.min(280,Math.round(viewportHeight*.26))):Math.max(260,viewportHeight-370)}} pagination={false} dataSource={items} onRow={row=>({onClick:()=>{if(interactive)void view(row);},style:{cursor:interactive?'pointer':'default',opacity:interactive?1:.65}})} columns={[
   {title:'名称',dataIndex:'name',width:280,render:(value:string,row:KubeRow)=><Button type="link" disabled={!interactive} style={{padding:0,maxWidth:260,overflow:'hidden',textOverflow:'ellipsis'}} onClick={e=>{e.stopPropagation();void view(row);}}>{value}</Button>},
   {title:'命名空间',dataIndex:'namespace',width:160,render:(value:string)=><Tag data-testid="kube-namespace-tag" color={namespaceColor(value)}>{value||'集群级'}</Tag>},
   {title:'状态',dataIndex:'status',width:200,render:(value:string)=><Tag data-testid="kube-status-tag" color={statusColor(value)}>{value||'—'}</Tag>},{title:'创建时间',dataIndex:'created',width:240}
  ]}/><Space size={8} style={{marginTop:8}}><Button disabled={!interactive||!!browserState.pending||shownPage===0} onClick={()=>changePage(shownPage-1)}>上一页</Button><Button disabled={!interactive||!!browserState.pending||!hasNext} onClick={()=>changePage(shownPage+1)}>下一页</Button></Space></Card>
  {selected&&<Card size="small" className="data-card" title={`${selectedResource??''} / ${selected.namespace||'集群级'} / ${selected.name}`}><Tabs items={[
   {key:'yaml',label:selectedResource==='secrets'?'元数据 YAML':'详情 / YAML',children:detail?<Input.TextArea aria-label="资源YAML只读详情" readOnly value={detail.yaml} autoSize={{minRows:12,maxRows:24}} style={mono}/>:<Typography.Text>正在读取详情…</Typography.Text>},
   ...(selectedResource==='pods'?[{key:'logs',label:'容器日志',children:<Space orientation="vertical" style={{width:'100%'}}><Space wrap>
    <Select aria-label="Pod容器" value={container} options={(detail?.containers??selected.containers).map(value=>({value,label:value}))} onChange={value=>{void stopLogs();setContainer(value);}} style={{width:240}}/>
    <Checkbox checked={follow} onChange={e=>setFollow(e.target.checked)}>跟随</Checkbox><Button onClick={()=>void startLogs()} disabled={!container||!!logId||startingLogs||!interactive}>读取日志</Button><Button onClick={()=>void stopLogs()} disabled={!logId&&!startingLogs}>停止日志</Button><Typography.Text type="secondary">{logStatus}</Typography.Text><Input.Search aria-label="搜索已加载日志" placeholder="搜索已加载日志" value={logSearch} onChange={e=>setLogSearch(e.target.value)} style={{width:240}} allowClear/>
   </Space>{dropped>0&&<Alert type="warning" title="输出已截断，保留最近日志"/>}<pre aria-label="Pod容器日志" style={{...mono,margin:0,padding:16,minHeight:200,maxHeight:420,overflow:'auto',background:'#101820',color:'#d8e6ef',borderRadius:6}}>{filteredLogs||'暂无日志'}</pre></Space>}]:[])
  ]}/></Card>}
 </Space>;
}
