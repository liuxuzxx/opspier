import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {CaretDownOutlined,CaretRightOutlined} from '@ant-design/icons';
import {Alert,Button,Empty,List,Modal,Select,Space,Tag,Typography,message} from 'antd';
import SshPanel,{type SshSession} from './SshPanel';
import DbPanel from './DbPanel';
import KubePanel from './KubePanel';
import MinioPanel from './MinioPanel';
import MinioDialog from './MinioDialog';
import type {MinioSession} from './minio-model';
import ConnectionDialog from './ConnectionDialog';
import KubeImportDialog from './KubeImportDialog';
import {categoryFor,legacyProfiles,type Profile,type ProfileListing,type MutationReceipt} from './profiles-model';
import {formatEndpoint} from './model';
import type {DbSession} from './db-model';
import type {KubeSelection} from './kube-model';
type Workspace={id:string;profileId:string;name:string}&({kind:'ssh';session:SshSession}|{kind:'db';session:DbSession;endpoint:string}|{kind:'kubernetes';selection:KubeSelection}|{kind:'minio';session:MinioSession});
type Probe={token:string;host:string;port:number;fingerprint:string;trusted:boolean};
type Dialog={profile:Profile|null;driver:'ssh'|'mysql'|'doris'|'kubernetes'|'minio'};
export default function ConnectionManager({sidebarCollapsed=false}:{sidebarCollapsed?:boolean}){
 const [collapsedCategories,setCollapsedCategories]=useState<Record<string,boolean>>({});
 function toggleCategory(category:string){setCollapsedCategories(previous=>({...previous,[category]:!previous[category]}));}
 const [listing,setListing]=useState<ProfileListing>({profiles:[],cleanupPending:0});const [error,setError]=useState('');const [notice,setNotice]=useState('');
 const [dialog,setDialog]=useState<Dialog|null>(null);const [newDriver,setNewDriver]=useState<'ssh'|'mysql'|'doris'|'kubernetes'|'minio'>('ssh');
 const [workspaces,setWorkspaces]=useState<Workspace[]>([]);const [active,setActive]=useState('');const [opening,setOpening]=useState<string[]>([]);
 const [contextPrompt,setContextPrompt]=useState<{profile:Profile;force:boolean;context?:string}|null>(null);
 const [toast,toastHolder]=message.useMessage();
 const [modal,holder]=Modal.useModal();const inFlight=useRef(new Set<string>());const mounted=useRef(true);const currentWorkspaces=useRef(workspaces);currentWorkspaces.current=workspaces;
 async function refresh(){const value=await invoke<ProfileListing>('profiles_list');if(mounted.current)setListing(value);}
 useEffect(()=>{
  mounted.current=true;
  void (async()=>{
   const load=async()=>{try{await refresh();}catch(e){if(mounted.current)setError(String(e));}};
   // Existing saved profiles are available even if an unrelated legacy record cannot migrate.
   await load();
   const legacy=legacyProfiles(localStorage);let failed=legacy.invalidCount;
   for(const profile of legacy.profiles){
    try{await invoke('profiles_import_legacy',{profiles:[profile]});}catch{failed++;}
   }
   if(failed&&mounted.current)setNotice(`有 ${failed} 项旧连接未迁移，原记录已保留；可调整名称或字段后手动新建。其余连接仍可使用。`);
   if(legacy.profiles.length)await load();
  })();
  return()=>{mounted.current=false;};
 },[]);
 function saved(receipt:MutationReceipt){
  setDialog(null);setError('');
  const result=receipt.profile?'连接配置已保存':'连接配置已删除';
  if(!receipt.durabilityConfirmed)setNotice(`${result}，但磁盘持久性尚未确认。请刷新检查，勿重复新建；旧凭据将保留至持久性确认。`);
  else if(receipt.cleanupPending)setNotice(`${result}，部分旧凭据待清理；可点击重试清理。`);
  else void toast.success(result);
  void refresh().catch(e=>setError(String(e)));
 }
 async function closeRuntime(workspace:Workspace){
  if(workspace.kind==='ssh')await invoke('ssh_disconnect',{id:workspace.id});
  else if(workspace.kind==='db')await invoke('db_disconnect',{sessionId:workspace.id});
  else if(workspace.kind==='minio')await invoke('minio_close',{sessionId:workspace.id});
  else await invoke('kube_close',{sessionId:workspace.id,generation:workspace.selection.generation});
 }
 function removeWorkspace(id:string){setWorkspaces(old=>old.filter(w=>w.id!==id));setActive(old=>old===id?(currentWorkspaces.current.find(w=>w.id!==id)?.id??''):old);}
 async function closeWorkspace(workspace:Workspace){try{await closeRuntime(workspace);removeWorkspace(workspace.id);}catch(e){setError(String(e));}}
 async function openProfile(profile:Profile,force=false,context?:string){
  const existing=currentWorkspaces.current.find(w=>w.profileId===profile.id);if(existing&&!force){setActive(existing.id);return;}
  if(inFlight.current.has(profile.id))return;
  const settings=profile.settings;
  if(!profile.credentialSaved){setDialog({profile,driver:settings.driver});setError('此连接尚未保存凭据，请补充后保存。');return;}
  if(settings.driver==='kubernetes'&&!context&&!settings.context){setContextPrompt({profile,force});return;}
  inFlight.current.add(profile.id);setOpening([...inFlight.current]);setError('');
  let workspace:Workspace|undefined;
  try{
   const args={id:profile.id,revision:profile.revision};
   if(settings.driver==='ssh'){
    const probe=await invoke<Probe>('profiles_probe_ssh',args);if(!mounted.current)return;
    let trust=probe.trusted;
    if(!trust)trust=await new Promise<boolean>(resolve=>modal.confirm({title:'核验 SSH 主机指纹',width:580,content:<Space orientation="vertical"><Typography.Text>{probe.host}:{probe.port}</Typography.Text><Typography.Text code copyable>{probe.fingerprint}</Typography.Text><Typography.Paragraph>请通过可信渠道比对主机 SHA256 指纹，核验后才会发送保存的凭据。</Typography.Paragraph></Space>,okText:'已核验并信任',cancelText:'取消连接',onOk:()=>resolve(true),onCancel:()=>resolve(false)}));
    if(!trust||!mounted.current)return;
    const id=await invoke<string>('profiles_connect_ssh',{...args,token:probe.token,trustHost:trust});
    workspace={id,profileId:profile.id,name:profile.name,kind:'ssh',session:{id,name:profile.name,endpoint:{host:settings.host,port:settings.port,username:settings.username},status:'connected'}};
   }else if(settings.driver==='mysql'||settings.driver==='doris'){
    const session=await invoke<DbSession>('profiles_connect_db',args);
    workspace={id:session.id,profileId:profile.id,name:profile.name,kind:'db',session,endpoint:formatEndpoint(settings.username,settings.host,settings.port)};
   }else if(settings.driver==='minio'){
    const session=await invoke<MinioSession>('profiles_connect_minio',args);
    workspace={id:session.id,profileId:profile.id,name:profile.name,kind:'minio',session};
   }else{
    const selection=await invoke<KubeSelection>('profiles_open_kube',{...args,context:context??null});
    workspace={id:selection.sessionId,profileId:profile.id,name:profile.name,kind:'kubernetes',selection};
   }
   if(!mounted.current){await closeRuntime(workspace);return;}
   const opened=workspace;setWorkspaces(old=>[...old,opened]);setActive(opened.id);setContextPrompt(null);
  }catch(e){if(mounted.current){setError(String(e));void refresh().catch(()=>{});}}
  finally{inFlight.current.delete(profile.id);if(mounted.current)setOpening([...inFlight.current]);}
 }
 async function deleteProfile(profile:Profile){
  modal.confirm({title:`删除连接「${profile.name}」？`,content:'删除保存的连接和对应凭据。已打开的工作区继续运行，可单独关闭。',okText:'删除连接',okButtonProps:{danger:true},onOk:async()=>{try{const receipt=await invoke<MutationReceipt>('profiles_remove',{id:profile.id,revision:profile.revision});saved(receipt);}catch(e){setError(String(e));await refresh();}}});
 }
 const activeWorkspace=workspaces.find(w=>w.id===active);const sshWorkspaces=workspaces.filter((w):w is Workspace&{kind:'ssh';session:SshSession}=>w.kind==='ssh');
 return <div className="connection-manager">{holder}{toastHolder}
 <aside id="connection-sidebar" className="connection-sidebar" hidden={sidebarCollapsed}><Typography.Title level={4} style={{marginTop:0}}>连接</Typography.Title><Space.Compact block><Select aria-label="新建连接类型" value={newDriver} onChange={setNewDriver} style={{flex:1}} options={[{value:'ssh',label:'SSH'},{value:'mysql',label:'MySQL'},{value:'doris',label:'Apache Doris'},{value:'kubernetes',label:'Kubernetes'},{value:'minio',label:'MinIO'}]}/><Button type="primary" onClick={()=>{setError('');setDialog({profile:null,driver:newDriver});}}>新建</Button></Space.Compact>
 {['主机与终端','数据库','Kubernetes','对象存储'].map((category,index)=><div key={category} className="profile-category"><button className="category-heading" aria-label={`${collapsedCategories[category]?'展开':'收起'}${category}`} aria-expanded={!collapsedCategories[category]} aria-controls={`profile-category-${index}`} onClick={()=>toggleCategory(category)}>{collapsedCategories[category]?<CaretRightOutlined/>:<CaretDownOutlined/>}<span>{category}</span></button><div id={`profile-category-${index}`} className="category-content" hidden={!!collapsedCategories[category]}><List dataSource={listing.profiles.filter(p=>categoryFor(p.settings.driver)===category)} locale={{emptyText:'暂无连接'}} renderItem={profile=><List.Item className={`profile-item ${activeWorkspace?.profileId===profile.id?'active':''}`}><div className="profile-main"><Button type="text" className="profile-name" onClick={()=>void openProfile(profile)} loading={opening.includes(profile.id)} title="打开连接">{profile.name}</Button><div className="muted">{profile.settings.driver==='kubernetes'?`${profile.settings.contexts.length} 个 Context`:profile.settings.driver==='minio'?profile.settings.endpoint:formatEndpoint(profile.settings.username,profile.settings.host,profile.settings.port)}</div><div className="profile-actions"><Tag color={profile.credentialSaved?'green':'default'}>{profile.credentialSaved?'凭据已保存':'待补凭据'}</Tag><Button size="small" type="text" onClick={()=>setDialog({profile,driver:profile.settings.driver})}>编辑</Button><Button size="small" type="text" danger onClick={()=>void deleteProfile(profile)}>删除</Button><Button size="small" type="text" onClick={()=>void openProfile(profile,true)} disabled={opening.includes(profile.id)}>新开</Button></div></div></List.Item>}/></div></div>)}
 <Typography.Paragraph type="secondary" className="sidebar-note">连接信息与运行工作区分别管理。切换连接会保留已打开的工作区。</Typography.Paragraph>
 <Button type="link" size="small" onClick={()=>modal.info({title:'驱动规划',content:<Space orientation="vertical"><Typography.Text>当前支持 SSH、MySQL、Apache Doris、Kubernetes 与 MinIO。</Typography.Text><Typography.Text>数据库：Oracle、SQL Server、SQLite；搜索：Elasticsearch；对象存储：RustFS；文件：NFS；时序数据库单独规划。</Typography.Text><Typography.Text>Apache Doris 归为分析型数据库。</Typography.Text></Space>})}>查看驱动规划</Button></aside>
 <main className="workspace-area">{error&&<Alert type="error" title={error} closable onClose={()=>setError('')}/>} {notice&&<Alert type="warning" title={notice} closable onClose={()=>setNotice('')}/>}
 {listing.cleanupPending>0&&<Alert type="warning" title={`有 ${listing.cleanupPending} 项旧凭据待清理`} action={<Button size="small" onClick={()=>void invoke<number>('profiles_retry_cleanup').then(async count=>{if(count)setNotice('部分旧凭据仍待清理，请解锁系统凭据服务后重试。');else{void toast.success('旧凭据已清理');setNotice(previous=>previous.includes('凭据')&&!previous.includes('旧连接')?'':previous);}await refresh();}).catch(e=>setError(String(e)))}>重试清理</Button>}/>}
 <div className="workspace-tabs" role="tablist" aria-label="运行工作区">{workspaces.map(workspace=><Space.Compact key={workspace.id}><Button role="tab" aria-selected={active===workspace.id} type={active===workspace.id?'primary':'default'} onClick={()=>setActive(workspace.id)}>{workspace.name}<Tag>{workspace.kind==='db'?workspace.session.kind:workspace.kind==='kubernetes'?'K8s':workspace.kind==='minio'?'MinIO':'SSH'}</Tag></Button><Button aria-label={`关闭工作区 ${workspace.name}`} onClick={()=>void closeWorkspace(workspace)}>×</Button></Space.Compact>)}</div>
 {!workspaces.length&&<div className="workspace-empty"><Empty description="从左侧新建或打开连接"/><Typography.Paragraph type="secondary">凭据使用系统凭据服务保存；打开连接时按需读取。</Typography.Paragraph></div>}
 {!!sshWorkspaces.length&&<section hidden={activeWorkspace?.kind!=='ssh'} className="workspace ssh-workspace"><SshPanel sessions={sshWorkspaces.map(w=>w.session)} selected={activeWorkspace?.kind==='ssh'?active:''} onClose={id=>{const w=workspaces.find(w=>w.id===id);if(w)void closeWorkspace(w);}}/></section>}
 {workspaces.map(workspace=>workspace.kind==='ssh'?null:<section key={workspace.id} hidden={active!==workspace.id} className="workspace scroll-panel">{workspace.kind==='db'?<DbPanel initialSession={workspace.session} endpoint={workspace.endpoint} onClose={()=>removeWorkspace(workspace.id)}/>:workspace.kind==='kubernetes'?<KubePanel initialSelection={workspace.selection}/>:<MinioPanel session={workspace.session} active={active===workspace.id&&!dialog&&!contextPrompt}/>}</section>)}
 </main>
 {dialog&&(dialog.driver==='minio'?<MinioDialog key={`${dialog.profile?.id??'new'}:minio`} profile={dialog.profile} onClose={()=>setDialog(null)} onSaved={saved}/>:dialog.driver==='kubernetes'?<KubeImportDialog key={`${dialog.profile?.id??'new'}:kube`} profile={dialog.profile} onClose={()=>setDialog(null)} onSaved={saved}/>:<ConnectionDialog key={`${dialog.profile?.id??'new'}:${dialog.driver}`} profile={dialog.profile} driver={dialog.driver} onClose={()=>setDialog(null)} onSaved={saved}/>)}
 <Modal open={!!contextPrompt} title="选择要打开的 Context" onCancel={()=>setContextPrompt(null)} okText="打开集群" okButtonProps={{disabled:!contextPrompt?.context,loading:contextPrompt?opening.includes(contextPrompt.profile.id):false}} onOk={()=>{if(contextPrompt?.context)void openProfile(contextPrompt.profile,contextPrompt.force,contextPrompt.context);}}><Select aria-label="打开集群Context" style={{width:'100%'}} value={contextPrompt?.context} onChange={context=>setContextPrompt(old=>old?{...old,context}:null)} options={contextPrompt?.profile.settings.driver==='kubernetes'?contextPrompt.profile.settings.contexts.map(value=>({value,label:value})):[]}/></Modal>
 </div>;
}
