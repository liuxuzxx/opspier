import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {getCurrentWebview} from '@tauri-apps/api/webview';
import {open as chooseFile} from '@tauri-apps/plugin-dialog';
import {Alert,Button,Col,Form,Input,Modal,Row,Select,Space,Tag,Tooltip,Typography} from 'antd';
import type {KubeSettings,MutationReceipt,Profile} from './profiles-model';
type Binding={draftId:string;id:string|null;expectedRevision:number|null};
type Source={kind:'file';path:string}|{kind:'text';text:string};
type Preview={token:string;settings:KubeSettings};
export default function KubeImportDialog({profile,onClose,onSaved}:{profile:Profile|null;onClose:()=>void;onSaved:(receipt:MutationReceipt)=>void}){
 const [form]=Form.useForm<{name:string;context:string;namespace:string}>();const [sourceKind,setSourceKind]=useState<'file'|'text'>('file');
 const [path,setPath]=useState('');const [text,setText]=useState('');const [preview,setPreview]=useState<Preview|null>(null);
 const [replace,setReplace]=useState(!profile?.credentialSaved);const [busy,setBusy]=useState(false);const [error,setError]=useState('');const [drag,setDrag]=useState(false);
 const binding=useRef<Binding>({draftId:crypto.randomUUID(),id:profile?.id??null,expectedRevision:profile?.revision??null});
 const [testing,setTesting]=useState(false);const [testResult,setTestResult]=useState<{kind:'success'|'error';text:string}|null>(null);const testRevision=useRef(0);
 function invalidateTest(){testRevision.current++;setTestResult(null);}
 const working=useRef(false);const mounted=useRef(true);const changed=useRef(0);
 const original=profile?.settings.driver==='kubernetes'?profile.settings:null;const settings=replace?preview?.settings??null:original;
 const contextInitialized=useRef(!!original);
 function invalidateSource(){invalidateTest();changed.current++;contextInitialized.current=false;setPreview(null);form.setFieldsValue({context:undefined,namespace:''});}
 useEffect(()=>{form.setFieldsValue({name:profile?.name??'',context:original?.context??undefined,namespace:original?.namespace??''});return()=>{mounted.current=false;changed.current++;setText('');void invoke('profiles_discard_import',{binding:binding.current}).catch(()=>{});};},[]);
 useEffect(()=>{
  let disposed=false;let unlisten:(()=>void)|undefined;
  void getCurrentWebview().onDragDropEvent(event=>{
   if(!mounted.current||working.current||!replace)return;
   if(event.payload.type==='enter'||event.payload.type==='over')setDrag(true);
   else {setDrag(false);if(event.payload.type==='drop'){if(event.payload.paths.length!==1){setError('请一次拖入一个配置文件');return;}invalidateSource();setSourceKind('file');setPath(event.payload.paths[0]);setError('');}}
  }).then(fn=>{if(disposed)fn();else unlisten=fn;}).catch(()=>{if(!disposed)setError('文件拖入监听失败，仍可选择文件或粘贴配置');});
  return()=>{disposed=true;unlisten?.();};
 },[replace]);
 async function selectFile(){try{const selected=await chooseFile({multiple:false,directory:false,title:'选择 kubeconfig 文件'});if(typeof selected==='string'){invalidateSource();setPath(selected);setSourceKind('file');}}catch{setError('文件选择失败，请重试或粘贴配置');}}
 async function parseSource():Promise<Preview>{
  const source:Source=sourceKind==='file'?{kind:'file',path}:{kind:'text',text};
  const result=await invoke<Preview>('profiles_import_kube',{binding:binding.current,source});
  return result;
 }
 async function inspect(){if(working.current)return;working.current=true;invalidateTest();setBusy(true);setError('');const request=changed.current;try{const result=await parseSource();if(mounted.current&&request===changed.current){contextInitialized.current=true;setPreview(result);form.setFieldsValue({context:result.settings.context??undefined,namespace:result.settings.namespace??''});}}catch(e){if(mounted.current)setError(String(e));}finally{working.current=false;if(mounted.current)setBusy(false);}}
 async function save(){
  if(working.current)return;working.current=true;setBusy(true);setError('');
  try{
   const value=await form.validateFields();let imported:Preview|null=null;
   // Fresh one-time token each save permits retry after OS-keyring failure or TTL expiry.
   if(replace)imported=await parseSource();
   const base=imported?.settings??original;if(!base)throw '请先导入集群配置。';
   const next={...base,driver:'kubernetes',context:contextInitialized.current?(value.context??null):base.context,namespace:value.namespace?.trim()||null};
   const receipt=await invoke<MutationReceipt>('profiles_save',{draft:{id:profile?.id??null,expectedRevision:profile?.revision??null,name:value.name.trim(),settings:next},credential:imported?{action:'importKube',token:imported.token,binding:binding.current}:{action:'keep'}});
   setText('');if(mounted.current)onSaved(receipt);
  }catch(e){if(mounted.current&&!(e&&typeof e==='object'&&'errorFields'in e))setError(String(e));}
  finally{working.current=false;if(mounted.current)setBusy(false);}
 }
 async function testConnection(){
  if(profile||working.current)return;working.current=true;setBusy(true);setTesting(true);setTestResult(null);const revision=testRevision.current;
  try{
   const value=await form.validateFields(['context','namespace']);if(!mounted.current||revision!==testRevision.current)return;
   const source:Source=sourceKind==='file'?{kind:'file',path}:{kind:'text',text};
   await invoke('kube_test_connection',{source,context:value.context??null,namespace:value.namespace?.trim()||null,useSourceDefault:!contextInitialized.current});
   if(mounted.current&&revision===testRevision.current)setTestResult({kind:'success',text:'测试连接成功，尚未保存'});
  }catch(e){if(mounted.current&&revision===testRevision.current&&!(e&&typeof e==='object'&&'errorFields'in e))setTestResult({kind:'error',text:String(e)});}
  finally{working.current=false;if(mounted.current){setBusy(false);setTesting(false);}}
 }
 function cancel(){if(working.current)return;setText('');onClose();}
 return <Modal open className="connection-dialog" width={760} title={profile?'编辑 Kubernetes 连接':'新建 Kubernetes 连接'} onCancel={cancel} closable={!busy} keyboard={!busy} maskClosable={!busy} footer={<Space>{!profile&&<Button onClick={()=>void testConnection()} loading={testing} disabled={busy&&!testing}>测试连接</Button>}<Button onClick={cancel} disabled={busy}>取消</Button><Button type="primary" onClick={()=>void save()} loading={busy&&!testing} disabled={testing}>保存集群</Button></Space>}>
 {testResult&&<Typography.Paragraph data-testid="connection-test-result" type={testResult.kind==='error'?'danger':'success'} style={{marginBottom:8}}>{testResult.text}</Typography.Paragraph>}
 {error&&<Alert type="error" title={error} style={{marginBottom:16}}/>}
 <Form form={form} layout="vertical" disabled={busy} onValuesChange={invalidateTest}>
 <Form.Item name="name" label="显示名称" rules={[{required:true,whitespace:true,max:80}]}><Input autoFocus placeholder="例如 测试集群" autoComplete="off"/></Form.Item>
 {original&&<Form.Item label="集群配置"><Select value={replace?'replace':'keep'} onChange={v=>{setReplace(v==='replace');invalidateSource();if(v==='keep'){contextInitialized.current=true;form.setFieldsValue({context:original?.context??undefined,namespace:original?.namespace??''});}}} options={[{value:'keep',label:'保留已保存配置'},{value:'replace',label:'重新导入并替换配置'}]}/></Form.Item>}
 {replace&&<><Space style={{marginBottom:12}}><Select aria-label="导入方式" value={sourceKind} onChange={value=>{invalidateSource();setSourceKind(value);}} options={[{value:'file',label:'选择或拖入文件'},{value:'text',label:'粘贴配置文本'}]}/>{sourceKind==='file'&&<Button onClick={()=>void selectFile()} disabled={busy}>选择文件</Button>}</Space>
 {sourceKind==='file'?<div className={`kube-drop compact-drop ${drag?'dragging':''}`}><Input aria-label="kubeconfig文件路径" value={path} onChange={e=>{invalidateSource();setPath(e.target.value);}} placeholder="选择文件或拖入到此窗口" autoComplete="off"/><Tooltip title="相对证书、密钥和token文件按源文件父目录解析；保存为自包含安全快照。"><span className="muted">可拖入文件 · 最多1MiB</span></Tooltip></div>:<Input.TextArea aria-label="kubeconfig配置文本" value={text} onChange={e=>{invalidateSource();setText(e.target.value);}} autoSize={{minRows:5,maxRows:9}} autoComplete="off" spellCheck={false} placeholder="粘贴 kubeconfig；相对文件引用请改用文件导入"/>}
 <Button style={{marginBlock:12}} onClick={()=>void inspect()} disabled={busy||(sourceKind==='file'?!path.trim():!text.trim())}>解析配置</Button></>}
 {settings&&<><Space size={8} style={{marginBottom:8}}><Tag color="green">{settings.contexts.length} 个 Context</Tag><Tooltip title="显示名称不改变内部cluster/context/user；配置材料保存在系统凭据服务，不执行外部认证插件。"><span className="muted">导入说明</span></Tooltip></Space><Row gutter={12}><Col xs={24} sm={12}><Form.Item name="context" label="默认 Context（可稍后选择）"><Select allowClear onChange={()=>{contextInitialized.current=true;}} options={settings.contexts.map(value=>({value,label:value}))}/></Form.Item></Col><Col xs={24} sm={12}><Form.Item name="namespace" label="默认命名空间（可选）"><Input maxLength={63}/></Form.Item></Col></Row></>}

 </Form></Modal>;
}
