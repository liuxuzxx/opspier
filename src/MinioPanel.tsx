import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {getCurrentWebview} from '@tauri-apps/api/webview';
import {open as chooseFile,save as chooseSave} from '@tauri-apps/plugin-dialog';
import {Alert,Button,Input,Modal,Progress,Select,Space,Table,Tag,Typography} from 'antd';
import {minioScopeKey,parentPrefix,uploadFileName,formatObjectSize,type MinioObject,type MinioPage,type MinioSession,type MinioTransfer} from './minio-model';
export default function MinioPanel({session,active}:{session:MinioSession;active:boolean}){
 const [buckets,setBuckets]=useState<string[]>(session.bucket?[session.bucket]:[]);const [bucket,setBucket]=useState(session.bucket);const [prefix,setPrefix]=useState('');const [prefixInput,setPrefixInput]=useState('');const [cursor,setCursor]=useState('');const cursors=useRef<string[]>([]);
 const [page,setPage]=useState<(MinioPage&{scope:string})|null>(null);const [loading,setLoading]=useState(false);const [error,setError]=useState('');const [refresh,setRefresh]=useState(0);
 const [busy,setBusy]=useState(false);const [drag,setDrag]=useState(false);const [transfer,setTransfer]=useState<MinioTransfer|null>(null);const [notice,setNotice]=useState('');
 const activeRef=useRef(active);activeRef.current=active;const dropDestination=useRef('');dropDestination.current=JSON.stringify([bucket,prefix]);
 const mounted=useRef(true);const working=useRef(false);const loadGeneration=useRef(0);const wantedRef=useRef('');wantedRef.current=minioScopeKey(bucket,prefix,cursor);const [modal,holder]=Modal.useModal();
 useEffect(()=>{
  void invoke<string[]>('minio_buckets',{sessionId:session.id}).then(items=>{if(mounted.current){setBuckets(items);setBucket(old=>old||items[0]||'');}}).catch(e=>{if(mounted.current)setError(String(e));});
  return()=>{mounted.current=false;loadGeneration.current++;};
 },[]);
 useEffect(()=>{if(bucket)void loadPage(bucket,prefix,cursor);},[bucket,prefix,cursor,refresh]);
 async function loadPage(targetBucket:string,targetPrefix:string,targetCursor:string){
  const scope=minioScopeKey(targetBucket,targetPrefix,targetCursor);const generation=++loadGeneration.current;setLoading(true);setError('');
  try{const result=await invoke<MinioPage>('minio_objects',{sessionId:session.id,bucket:targetBucket,prefix:targetPrefix,cursor:targetCursor||null});if(mounted.current&&generation===loadGeneration.current&&scope===wantedRef.current)setPage({...result,scope});}
  catch(e){if(mounted.current&&generation===loadGeneration.current&&scope===wantedRef.current)setError(String(e));}
  finally{if(mounted.current&&generation===loadGeneration.current)setLoading(false);}
 }
 function navigate(targetBucket:string,targetPrefix:string){setBucket(targetBucket);setPrefix(targetPrefix);setPrefixInput(targetPrefix);setCursor('');cursors.current=[];}
 async function askOverwrite(key:string):Promise<boolean>{return new Promise(resolve=>modal.confirm({title:'覆盖同名对象？',content:<Typography.Text style={{overflowWrap:'anywhere'}}>{key}</Typography.Text>,okText:'覆盖对象',cancelText:'取消上传',okButtonProps:{danger:true},onOk:()=>resolve(true),onCancel:()=>resolve(false)}));}
 async function uploadOne(path:string,targetBucket:string,targetPrefix:string):Promise<MinioTransfer|null>{
  const name=uploadFileName(path);const args={sessionId:session.id,bucket:targetBucket,prefix:targetPrefix,path,overwrite:false};
  try{return await invoke<MinioTransfer>('minio_upload',args);}catch(e){if(String(e)!=='OBJECT_EXISTS')throw e;if(!await askOverwrite(targetPrefix+name))return null;return invoke<MinioTransfer>('minio_upload',{...args,overwrite:true});}
 }
 async function followTransfer(start:MinioTransfer){
  let value=start;
  while(mounted.current){setTransfer(value);if(value.status!=='running')return value;await new Promise(resolve=>setTimeout(resolve,400));value=await invoke<MinioTransfer>('minio_transfer',{sessionId:session.id,transferId:value.id});}
  return value;
 }
 async function uploadPaths(paths:string[]){
  if(working.current||!bucket||!paths.length)return;working.current=true;setBusy(true);setError('');setNotice('');const targetBucket=bucket;const targetPrefix=prefix;
  try{
   if(paths.length>100)throw '一次最多上传100个文件，请分批拖入。';let completed=0;let skipped=0;
   for(const path of paths){if(!mounted.current)return;const started=await uploadOne(path,targetBucket,targetPrefix);if(!started){skipped++;continue;}const result=await followTransfer(started);if(result.status!=='completed'){if(result.status==='failed')throw result.error||'文件上传失败';break;}completed++;}
   if(mounted.current)setNotice(`已上传 ${completed} 个文件${skipped?`，跳过 ${skipped} 个`:''}`);
  }catch(e){if(mounted.current)setError(String(e));}
  finally{working.current=false;if(mounted.current){setBusy(false);setRefresh(value=>value+1);}}
 }
 async function selectUpload(){try{const files=await chooseFile({multiple:true,directory:false,title:'选择上传文件'});if(files)await uploadPaths(typeof files==='string'?[files]:files);}catch{if(mounted.current)setError('文件选择失败，请重试');}}
 useEffect(()=>{
  if(!active)return;let disposed=false;let unlisten:(()=>void)|undefined;
  void getCurrentWebview().onDragDropEvent(event=>{
   if(disposed||!activeRef.current||!mounted.current||working.current||dropDestination.current!==JSON.stringify([bucket,prefix]))return;
   if(event.payload.type==='enter'||event.payload.type==='over')setDrag(true);
   else {setDrag(false);if(event.payload.type==='drop')void uploadPaths(event.payload.paths);}
  }).then(fn=>{if(disposed)fn();else unlisten=fn;}).catch(()=>{if(!disposed)setError('文件拖入监听失败，仍可使用“上传文件”选择文件');});
  return()=>{disposed=true;unlisten?.();setDrag(false);};
 },[active,bucket,prefix]);
 async function download(object:MinioObject){
  if(working.current)return;working.current=true;setBusy(true);setError('');setNotice('');
  try{const name=object.key.split('/').pop()||'download';const path=await chooseSave({title:'保存 MinIO 文件',defaultPath:name});if(!path||!mounted.current)return;
   const start=await invoke<MinioTransfer>('minio_download',{sessionId:session.id,bucket,key:object.key,path});const result=await followTransfer(start);if(mounted.current){if(result.status==='completed')setNotice('文件已下载');else if(result.status==='failed')setError(result.error||'文件下载失败');}
  }catch(e){if(mounted.current)setError(String(e));}finally{working.current=false;if(mounted.current)setBusy(false);}
 }
 function remove(object:MinioObject){
  if(working.current)return;const targetBucket=bucket;
  modal.confirm({title:'删除此对象？',content:<Space orientation="vertical"><Typography.Text>{targetBucket}</Typography.Text><Typography.Text style={{overflowWrap:'anywhere'}}>{object.key}</Typography.Text></Space>,okText:'删除对象',cancelText:'取消',okButtonProps:{danger:true},onOk:async()=>{
   if(working.current)return;working.current=true;setBusy(true);setError('');setNotice('');
   try{await invoke('minio_delete',{sessionId:session.id,bucket:targetBucket,key:object.key});if(mounted.current){setNotice('对象已删除');setRefresh(v=>v+1);}}
   catch(e){if(mounted.current)setError(String(e));}finally{working.current=false;if(mounted.current)setBusy(false);}
  }});
 }
 const displayedScope=page?JSON.parse(page.scope) as [string,string,string]:null;const displayedPrefix=displayedScope?.[1]??prefix;
 const interactive=!!page&&page.scope===wantedRef.current&&!loading;const progress=transfer?.total?Math.min(100,Math.floor(transfer.bytes/transfer.total*100)):0;
 return <div className={`minio-panel ${drag?'dragging':''}`} data-testid="minio-panel">{holder}
 <div className="minio-toolbar"><Space wrap><Tag color="green">已连接</Tag><Typography.Text type="secondary">{session.endpoint}</Typography.Text><Select aria-label="MinIO桶" showSearch value={bucket||undefined} style={{minWidth:190}} options={buckets.map(value=>({value,label:value}))} onChange={value=>navigate(value,'')}/><Button loading={loading} disabled={!bucket} onClick={()=>setRefresh(v=>v+1)}>刷新对象</Button></Space><Button type="primary" disabled={busy||!bucket} onClick={()=>void selectUpload()}>上传文件</Button></div>
 <Space.Compact block><Button disabled={!prefix} onClick={()=>navigate(bucket,parentPrefix(prefix))}>上级目录</Button><Input.Search aria-label="对象前缀" placeholder="输入前缀后回车；可拖入文件到此窗口上传" value={prefixInput} onChange={e=>setPrefixInput(e.target.value)} onSearch={value=>navigate(bucket,value)} enterButton="打开前缀"/></Space.Compact>
 {page&&page.scope!==wantedRef.current&&<Alert type="info" title={`显示上次成功结果：${displayedScope?.[0]} / ${displayedPrefix}；当前选择尚未加载成功`}/>}
 {error&&<Alert type="error" title={error} closable onClose={()=>setError('')}/>} {notice&&<Typography.Text type="success">{notice}</Typography.Text>}
 {drag&&<div className="minio-drop-overlay">松开文件，上传到 {bucket} / {prefix}</div>}
 {transfer&&<div className="minio-transfer" data-testid="minio-transfer"><Space wrap><Tag color={transfer.status==='completed'?'green':transfer.status==='failed'?'red':transfer.status==='cancelled'?'default':'blue'}>{{running:'传输中',completed:'已完成',cancelled:'已取消',failed:'失败'}[transfer.status]}</Tag><Typography.Text ellipsis title={`${transfer.bucket}/${transfer.key}`}>{transfer.direction==='upload'?'上传':'下载'} {transfer.bucket}/{transfer.key}</Typography.Text><Typography.Text type="secondary">{formatObjectSize(transfer.bytes)} / {formatObjectSize(transfer.total)}</Typography.Text>{transfer.status==='running'&&<Button size="small" onClick={()=>void invoke('minio_cancel_transfer',{sessionId:session.id,transferId:transfer.id}).catch(e=>setError(String(e)))}>取消传输</Button>}</Space><Progress percent={transfer.status==='completed'?100:progress} size="small" status={transfer.status==='failed'?'exception':transfer.status==='completed'?'success':'normal'}/></div>}
 <Table<MinioObject> size="small" rowKey="key" loading={loading} dataSource={page?.objects??[]} pagination={false} scroll={{x:700}} locale={{emptyText:bucket?'此目录暂无对象':'此连接暂无可访问的桶'}} columns={[
 {title:'对象名称',dataIndex:'key',render:(_value:string,object:MinioObject)=>object.directory?<Button type="link" disabled={!interactive} onClick={()=>navigate(bucket,object.key)}>{object.key.slice(displayedPrefix.length)||object.key}</Button>:<Typography.Text style={{overflowWrap:'anywhere'}}>{object.key.startsWith(displayedPrefix)?object.key.slice(displayedPrefix.length):object.key}</Typography.Text>},
 {title:'类型',width:70,render:(_value:unknown,object:MinioObject)=><Tag color={object.directory?'blue':'default'}>{object.directory?'目录':'文件'}</Tag>},
 {title:'大小',width:110,render:(_value:unknown,object:MinioObject)=>object.directory?'—':formatObjectSize(object.size)},
 {title:'修改时间',dataIndex:'modified',width:210,render:(value:string|null)=>value?new Date(value).toLocaleString():'—'},
 {title:'操作',width:140,render:(_value:unknown,object:MinioObject)=>!object.directory&&<Space><Button size="small" disabled={!interactive||busy} onClick={()=>void download(object)}>下载</Button><Button danger size="small" disabled={!interactive||busy} onClick={()=>remove(object)}>删除</Button></Space>}
 ]}/>
 <div className="minio-pagination"><Typography.Text type="secondary">{displayedScope?`${displayedScope[0]} / ${displayedPrefix}`:bucket?`${bucket} / ${prefix}`:'选择桶'} · 本页 {page?.objects.length??0} 项</Typography.Text><Space><Button disabled={!interactive||!cursors.current.length} onClick={()=>setCursor(cursors.current.pop()??'')}>上一页</Button><Button disabled={!interactive||!page?.nextCursor} onClick={()=>{cursors.current.push(cursor);setCursor(page?.nextCursor??'');}}>下一页</Button></Space></div>
 </div>;
}
