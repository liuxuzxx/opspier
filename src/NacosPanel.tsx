import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {Alert,AutoComplete,Button,Drawer,Input,InputNumber,Modal,Select,Space,Switch,Table,Tabs,Tag,Typography} from 'antd';
import {NacosWorkspace} from './nacos-workspace';
import {nacosScopeKey,nacosConfigPattern,parseMetadata,type NacosSession,type NacosNamespace,type ConfigKey,type ConfigView,type ConfigItem,type ConfigHistory,type Page,type ServiceKey,type ServiceItem,type NacosServiceView,type NacosInstance} from './nacos-model';
const json=(value:Record<string,string>)=>JSON.stringify(value,null,2);
export default function NacosPanel({session,onBeforeClose}:{session:NacosSession;onBeforeClose?:(check:(()=>Promise<boolean>)|null)=>void}){
 const mounted=useRef(true);const [busy,setBusy]=useState(false);const gateRef=useRef<NacosWorkspace|null>(null);
 if(!gateRef.current)gateRef.current=new NacosWorkspace(value=>{if(mounted.current)setBusy(value);});const gate=gateRef.current;
 const [modal,modalHolder]=Modal.useModal();const [error,setError]=useState('');const [notice,setNotice]=useState('');
 const [namespace,setNamespace]=useState(session.namespace);const [namespaceInput,setNamespaceInput]=useState(session.namespace);const [namespaces,setNamespaces]=useState<NacosNamespace[]>([]);
 const [mode,setMode]=useState('configs');const [group,setGroup]=useState('');const [search,setSearch]=useState('');const [page,setPage]=useState(1);const [total,setTotal]=useState(0);
 const [configs,setConfigs]=useState<ConfigItem[]>([]);const [services,setServices]=useState<ServiceItem[]>([]);const [loading,setLoading]=useState(false);const listEpoch=useRef(0);
 const detailEpoch=useRef(0);const detailPending=useRef(false);const [detailLoading,setDetailLoading]=useState(false);
 const [key,setKey]=useState<ConfigKey|null>(null);const [original,setOriginal]=useState<ConfigView|null>(null);const [content,setContent]=useState('');const [configType,setConfigType]=useState('text');
 const [serviceView,setServiceView]=useState<NacosServiceView|null>(null);const [threshold,setThreshold]=useState(0);const [serviceMetadata,setServiceMetadata]=useState('{}');
 const [instance,setInstance]=useState<NacosInstance|null>(null);const [enabled,setEnabled]=useState(false);const [weight,setWeight]=useState(1);const [instanceMetadata,setInstanceMetadata]=useState('{}');
 const [history,setHistory]=useState<Page<ConfigHistory>|null>(null);const [preview,setPreview]=useState<ConfigView|null>(null);
 const dirty=!!key&&(original?content!==original.content||configType!==original.type:!!content||!!key.dataId)||!!serviceView&&(threshold!==serviceView.protectThreshold||serviceMetadata!==json(serviceView.metadata))||!!instance&&(enabled!==instance.enabled||weight!==instance.weight||instanceMetadata!==json(instance.metadata));
 const confirm=(title:string,body:string)=>new Promise<boolean>(resolve=>modal.confirm({title,content:body,zIndex:1200,okText:'确认',cancelText:'取消',onOk:()=>resolve(true),onCancel:()=>resolve(false)}));
 const leave=()=>dirty?confirm('放弃未保存的修改？','当前草稿不会自动发布。确认后切换目标。'):Promise.resolve(true);
 const leaveRef=useRef(leave);leaveRef.current=leave;
 useEffect(()=>{onBeforeClose?.(async()=>{if(gate.locked){setError('写入或确认正在进行，请完成后关闭工作区。');return false;}return leaveRef.current();});return()=>onBeforeClose?.(null);},[]);
 const describe=(value:ConfigKey|ServiceKey)=>`namespace=${value.namespaceId||'public'} / group=${value.group} / ${'dataId'in value?'Data ID='+value.dataId:'service='+value.name}`;
 function clear(){detailEpoch.current++;detailPending.current=false;setDetailLoading(false);setKey(null);setOriginal(null);setContent('');setConfigType('text');setServiceView(null);setInstance(null);setHistory(null);setPreview(null);setError('');setNotice('');}
 async function load(nextPage=page,ns=namespace,kind=mode){
  if(gate.locked)return;const epoch=++listEpoch.current;setLoading(true);setError('');
  try{
   if(kind==='configs'){
    const result=await gate.query(()=>invoke<Page<ConfigItem>>('nacos_configs',{sessionId:session.id,namespace:ns,group:nacosConfigPattern(group),dataId:nacosConfigPattern(search),page:nextPage,pageSize:20}));
    if(result&&epoch===listEpoch.current){setConfigs(result.items);setTotal(result.total);setPage(nextPage);}
   }else{
    const result=await gate.query(()=>invoke<Page<ServiceItem>>('nacos_services',{sessionId:session.id,namespace:ns,group,name:search,page:nextPage,pageSize:20}));
    if(result&&epoch===listEpoch.current){setServices(result.items);setTotal(result.total);setPage(nextPage);}
   }
  }catch(e){if(mounted.current&&epoch===listEpoch.current)setError(String(e));}
  finally{if(mounted.current&&epoch===listEpoch.current)setLoading(false);}
 }
 useEffect(()=>{
  void (async()=>{await gate.transition(nacosScopeKey(namespace,mode));if(!mounted.current)return;
   void load(1);try{const rows=await gate.query(()=>invoke<NacosNamespace[]>('nacos_namespaces',{sessionId:session.id}));if(rows)setNamespaces(rows);}catch(e){if(mounted.current)setError(String(e));}
  })();
  return()=>{mounted.current=false;gate.dispose();void invoke('nacos_close',{sessionId:session.id}).catch(()=>{});};
 },[]);
 async function changeScope(ns:string,kind=mode){
  const next=ns==='public'?'':ns.trim();if(!await gate.transition(nacosScopeKey(next,kind),leave))return;
  listEpoch.current++;setLoading(false);clear();setNamespace(next);setNamespaceInput(next);setMode(kind);setPage(1);setConfigs([]);setServices([]);await load(1,next,kind);
 }
 async function chooseConfig(target:ConfigKey){
  if(!await gate.transition(nacosScopeKey(target.namespaceId,target.group,target.dataId),leave))return;clear();setKey(target);
  const request=++detailEpoch.current;detailPending.current=true;setDetailLoading(true);
  try{const value=await gate.query(()=>invoke<ConfigView|null>('nacos_config',{sessionId:session.id,key:target}));if(request!==detailEpoch.current)return;if(value){setOriginal(value);setContent(value.content);setConfigType(value.type);}else if(value===null)setError('配置已不存在，请刷新列表。');}
  catch(e){if(mounted.current&&request===detailEpoch.current)setError(String(e));}
  finally{if(mounted.current&&request===detailEpoch.current){detailPending.current=false;setDetailLoading(false);}}
 }
 async function newConfig(){if(await gate.transition(nacosScopeKey(namespace,'new-config'),leave)){clear();setKey({namespaceId:namespace,group:group||'DEFAULT_GROUP',dataId:''});}}
 async function closeDetail(){if(await gate.transition(nacosScopeKey(namespace,mode),leave))clear();}
 async function chooseService(target:ServiceKey){
  if(!await gate.transition(nacosScopeKey(target.namespaceId,target.group,target.name),leave))return;clear();
  try{const value=await gate.query(()=>invoke<NacosServiceView>('nacos_service',{sessionId:session.id,key:target}));if(value){setServiceView(value);setThreshold(value.protectThreshold);setServiceMetadata(json(value.metadata));}}
  catch(e){if(mounted.current)setError(String(e));}
 }
 async function selectInstance(value:NacosInstance){
  if(!serviceView)return;
  const changed=!!instance&&(enabled!==instance.enabled||weight!==instance.weight||instanceMetadata!==json(instance.metadata));
  const scope=nacosScopeKey(serviceView.key.namespaceId,serviceView.key.group,serviceView.key.name,'instance',JSON.stringify(value.key));
  if(!await gate.transition(scope,()=>changed?confirm('放弃实例草稿？','确认后选择其他实例，未发布修改不会生效。'):Promise.resolve(true)))return;
  setInstance(value);setEnabled(value.enabled);setWeight(value.weight);setInstanceMetadata(json(value.metadata));
 }
 async function write<T>(title:string,target:string,work:()=>Promise<T>,apply:(value:T)=>void){
  setError('');setNotice('');
  try{const value=await gate.mutate(()=>confirm(title,target+'。写请求只发送一次，服务端读回核验后才显示成功。'),work);if(value!==undefined){apply(value);setNotice('服务端读回已核验。');void load(1);}}
  catch(e){if(mounted.current)setError(String(e));}
 }
 function publish(){if(detailPending.current)return;if(!key||!key.dataId.trim()){setError('请填写 Data ID。');return;}const target={...key};void write(original?'发布配置？':'新建配置？',describe(target),()=>invoke<ConfigView>('nacos_publish',{sessionId:session.id,key:target,content,configType,expectedMd5:original?.md5??null}),value=>{setOriginal(value);setContent(value.content);setConfigType(value.type);});}
 function remove(){if(!original)return;const value=original;void write('删除此配置？',describe(value.key),async()=>{await invoke('nacos_delete_config',{sessionId:session.id,key:value.key,expectedMd5:value.md5});return true;},()=>{clear();});}
 async function openHistory(nextPage=1){if(!original||gate.locked)return;try{const value=await gate.query(()=>invoke<Page<ConfigHistory>>('nacos_history',{sessionId:session.id,key:original.key,page:nextPage,pageSize:20}));if(value)setHistory(value);}catch(e){if(mounted.current)setError(String(e));}}
 async function compareHistory(id:string){if(!original||gate.locked)return;try{const value=await gate.query(()=>invoke<ConfigView>('nacos_history_config',{sessionId:session.id,key:original.key,historyId:id}));if(value)setPreview(value);}catch(e){if(mounted.current)setError(String(e));}}
 function rollback(id:string){if(!original)return;const value=original;void write('回滚至此历史版本？',describe(value.key)+(dirty?'；当前未发布草稿会被替换':''),()=>invoke<ConfigView>('nacos_rollback',{sessionId:session.id,key:value.key,historyId:id,expectedMd5:value.md5}),updated=>{setOriginal(updated);setContent(updated.content);setConfigType(updated.type);setHistory(null);setPreview(null);});}
 function saveService(){if(!serviceView)return;try{const metadata=parseMetadata(serviceMetadata);const current=serviceView;void write('更新此服务属性？',describe(current.key),()=>invoke<NacosServiceView>('nacos_update_service',{sessionId:session.id,key:current.key,expectedSnapshot:current.snapshot,protectThreshold:threshold,metadata}),updated=>{setServiceView(updated);setThreshold(updated.protectThreshold);setServiceMetadata(json(updated.metadata));});}catch(e){setError(String(e));}}
 function saveInstance(){if(!serviceView||!instance)return;try{const metadata=parseMetadata(instanceMetadata);const current=instance;const target=serviceView.key;void write('更新此实例？',describe(target)+` / ${current.key.cluster}/${current.key.ip}:${current.key.port} / ${current.key.ephemeral?'临时':'持久'}`,()=>invoke<NacosInstance>('nacos_update_instance',{sessionId:session.id,key:target,target:current.key,expectedSnapshot:current.snapshot,enabled,weight,metadata}),updated=>{setInstance(updated);setEnabled(updated.enabled);setWeight(updated.weight);setInstanceMetadata(json(updated.metadata));setServiceView(old=>old?{...old,instances:old.instances.map(i=>JSON.stringify(i.key)===JSON.stringify(updated.key)?updated:i)}:old);});}catch(e){setError(String(e));}}
 const detailOpen=mode==='configs'?!!key:!!serviceView;
 const feedback=<>{error&&<Alert type="error" title={error} showIcon closable onClose={()=>setError('')}/>}{notice&&<Alert type="success" title={notice} closable onClose={()=>setNotice('')}/>}</>;
 return <div className="nacos-panel">{modalHolder}
 <form className="workspace-toolbar nacos-toolbar" onSubmit={event=>{event.preventDefault();if(!busy&&!loading)void load(1);}}>
 <div className="nacos-connection-info"><Tag color="green">Nacos {session.version}</Tag><Typography.Text type="secondary" ellipsis={{tooltip:session.endpoint}}>{session.endpoint}</Typography.Text></div>
 <label className="nacos-inline-field nacos-namespace-field"><span>命名空间</span><AutoComplete aria-label="Nacos命名空间" value={namespaceInput} disabled={busy} options={namespaces.map(n=>({value:n.id||'public',label:`${n.name} (${n.id||'public'})`}))} onChange={setNamespaceInput} onSelect={value=>void changeScope(value)}><Input placeholder="public / namespace" onPressEnter={()=>void changeScope(namespaceInput)}/></AutoComplete></label>
 <label className="nacos-inline-field"><span>Group</span><AutoComplete aria-label="Nacos Group查询" value={group} options={[...new Set(configs.map(row=>row.key.group))].map(value=>({value}))} onChange={setGroup} onSelect={value=>setGroup(value)}><Input placeholder="Group 关键字" allowClear disabled={busy}/></AutoComplete></label>
 <label className="nacos-inline-field"><span>{mode==='configs'?'Data ID':'服务名'}</span><AutoComplete aria-label="Nacos名称查询" value={search} options={[...new Set((mode==='configs'?configs:services).map(row=>mode==='configs'?(row as ConfigItem).key.dataId:(row as ServiceItem).key.name))].map(value=>({value}))} onChange={setSearch} onSelect={value=>setSearch(value)}><Input placeholder={mode==='configs'?'Data ID 关键字':'服务名关键字'} allowClear disabled={busy}/></AutoComplete></label>
 <Button htmlType="submit" disabled={busy||loading} type="primary">查询</Button><Button disabled={busy||loading} onClick={()=>void load(1)}>刷新</Button>{mode==='configs'&&<Button onClick={()=>void newConfig()} disabled={busy}>新建</Button>}
 </form>
 {!detailOpen&&feedback}
 <Tabs activeKey={mode} onChange={value=>void changeScope(namespace,value)} items={[{key:'configs',label:'配置'},{key:'services',label:'服务与实例'}]}/>
 <div className="nacos-list"><Typography.Text type="secondary" className="nacos-search-hint">Group、Data ID 支持关键字搜索，输入后按回车或点击查询{mode==='configs'?'，支持 * 通配符':''}</Typography.Text>
 {mode==='configs'?<><Table<ConfigItem> tableLayout="fixed" size="small" loading={loading} dataSource={configs} rowKey={r=>nacosScopeKey(r.key.namespaceId,r.key.group,r.key.dataId)} pagination={{current:page,pageSize:20,total,showSizeChanger:false,onChange:value=>void load(value)}} columns={[{title:'Data ID',width:'55%',ellipsis:true,render:(_,r)=><Button className="nacos-config-link" type="link" title={r.key.dataId} disabled={busy} onClick={()=>void chooseConfig(r.key)}>{r.key.dataId}</Button>},{title:'Group',width:'33%',ellipsis:true,render:(_,r)=>r.key.group},{title:'类型',dataIndex:'type',width:'12%'}]}/></>:<Table<ServiceItem> size="small" loading={loading} dataSource={services} rowKey={r=>nacosScopeKey(r.key.namespaceId,r.key.group,r.key.name)} pagination={{current:page,pageSize:20,total,showSizeChanger:false,onChange:value=>void load(value)}} columns={[{title:'服务',render:(_,r)=><Button type="link" disabled={busy} onClick={()=>void chooseService(r.key)}>{r.key.name}</Button>},{title:'Group',render:(_,r)=>r.key.group},{title:'实例',dataIndex:'instanceCount'},{title:'健康',dataIndex:'healthyCount'}]}/>}
 </div><Drawer rootClassName="nacos-detail-drawer" title={mode==='configs'?(original?'编辑配置':'新建配置'):'服务与实例'} open={detailOpen} size="min(960px, calc(100vw - 32px))" closable={!busy} keyboard={!busy} mask={{closable:!busy}} onClose={()=>void closeDetail()}><div className="nacos-editor">{detailOpen&&feedback}{mode==='configs'&&key?<><Typography.Text code>{describe(key)}</Typography.Text><div className="nacos-config-fields"><Input aria-label="配置Group" value={key.group} disabled={busy||detailLoading||!!original} onChange={e=>setKey(old=>old?{...old,group:e.target.value}:old)}/><Input aria-label="配置Data ID" value={key.dataId} disabled={busy||detailLoading||!!original} placeholder="Data ID" onChange={e=>setKey(old=>old?{...old,dataId:e.target.value}:old)}/><Select aria-label="配置类型" value={configType} disabled={busy||detailLoading} style={{width:130}} onChange={setConfigType} options={['text','json','xml','yaml','html','properties','toml'].map(value=>({value,label:value}))}/></div>
 <Input.TextArea aria-label="Nacos配置内容" value={content} disabled={busy||detailLoading} onChange={e=>setContent(e.target.value)} spellCheck={false} style={{fontFamily:'monospace',height:'min(58vh,700px)',resize:'vertical'}}/>
 <Space style={{marginTop:10}}><Button type="primary" loading={busy} disabled={detailLoading} onClick={publish}>发布配置</Button><Button disabled={busy||!original} onClick={()=>void openHistory()}>配置历史</Button><Button danger disabled={busy||!original} onClick={remove}>删除配置</Button>{dirty&&<Tag color="gold">未发布草稿</Tag>}</Space>
 </>:mode==='services'&&serviceView?<><Typography.Text code>{describe(serviceView.key)}</Typography.Text><Space style={{margin:'10px 0'}}><Typography.Text>保护阈值</Typography.Text><InputNumber aria-label="服务保护阈值" min={0} max={1} step={0.1} value={threshold} disabled={busy} onChange={value=>setThreshold(value??0)}/><Button disabled={busy} onClick={saveService}>更新服务属性</Button></Space><Input.TextArea aria-label="服务元数据" value={serviceMetadata} disabled={busy} onChange={e=>setServiceMetadata(e.target.value)} rows={3}/>
 <Table<NacosInstance> size="small" dataSource={serviceView.instances} rowKey={r=>JSON.stringify(r.key)} pagination={{pageSize:10,showSizeChanger:false}} columns={[{title:'实例',render:(_,r)=><Button type="link" disabled={busy} onClick={()=>selectInstance(r)}>{r.key.ip}:{r.key.port}</Button>},{title:'Cluster',render:(_,r)=>r.key.cluster},{title:'状态',render:(_,r)=><><Tag color={r.enabled?'green':'default'}>{r.enabled?'启用':'停用'}</Tag><Tag color={r.healthy?'green':'red'}>{r.healthy?'健康':'不健康'}</Tag></>},{title:'权重',dataIndex:'weight'}]}/>
 {instance&&<><Space><Typography.Text>启用</Typography.Text><Switch aria-label="实例启用" checked={enabled} disabled={busy} onChange={setEnabled}/><Typography.Text>权重</Typography.Text><InputNumber aria-label="实例权重" min={0} max={10000} value={weight} disabled={busy} onChange={value=>setWeight(value??0)}/><Button disabled={busy} onClick={saveInstance}>更新实例</Button></Space><Input.TextArea aria-label="实例元数据" value={instanceMetadata} disabled={busy} onChange={e=>setInstanceMetadata(e.target.value)} rows={4} style={{marginTop:10}}/><Typography.Text type="secondary">元数据支持新增、修改；Nacos 2.x 覆盖合并，请保留现有键。Cluster、临时/持久属性保持，健康状态由服务端维护。</Typography.Text></>}
 </>:null}</div></Drawer>
 <Modal open={!!history} zIndex={1200} title="配置历史" width={1000} footer={null} closable={!busy} onCancel={()=>{if(!gate.locked){setHistory(null);setPreview(null);}}}>
 <Table<ConfigHistory> size="small" dataSource={history?.items} rowKey="id" pagination={{current:history?.page,pageSize:20,total:history?.total,showSizeChanger:false,onChange:value=>void openHistory(value)}} columns={[{title:'版本ID',dataIndex:'id'},{title:'时间',dataIndex:'modified'},{title:'操作',dataIndex:'operation'},{title:'',render:(_,r)=><Space><Button disabled={busy} onClick={()=>void compareHistory(r.id)}>对照</Button><Button danger disabled={busy} onClick={()=>rollback(r.id)}>回滚</Button></Space>}]}/>
 {preview&&<div className="nacos-columns"><div><Typography.Text>当前未发布草稿</Typography.Text><Input.TextArea aria-label="当前配置对照" value={content} readOnly rows={12}/></div><div><Typography.Text>历史内容</Typography.Text><Input.TextArea aria-label="历史配置对照" value={preview.content} readOnly rows={12}/></div></div>}
 </Modal></div>;
}
