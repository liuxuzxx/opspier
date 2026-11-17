import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {Alert,Button,Form,Input,Modal,Select,Space,Typography} from 'antd';
import {credentialTargetUnchanged,type MutationReceipt,type Profile} from './profiles-model';
import {nacosDefaults,type NacosSettings} from './nacos-model';
type Values=ReturnType<typeof nacosDefaults>;
export default function NacosDialog({profile,onClose,onSaved}:{profile:Profile|null;onClose:()=>void;onSaved:(value:MutationReceipt)=>void}){
 const [form]=Form.useForm<Values>();const [mode,setMode]=useState<'keep'|'replace'|'remove'>(profile?.credentialSaved?'keep':'replace');
 const [busy,setBusy]=useState(false);const [testing,setTesting]=useState(false);const [error,setError]=useState('');
 const [result,setResult]=useState<{kind:'success'|'error';text:string}|null>(null);const occupied=useRef(false);const mounted=useRef(true);const revision=useRef(0);
 const values=Form.useWatch([],form) as Values|undefined;
 useEffect(()=>{form.setFieldsValue({...nacosDefaults(),...(profile?.settings.driver==='nacos'?{...profile.settings,name:profile.name}:{})});return()=>{mounted.current=false;form.setFieldsValue({username:'',password:''});};},[]);
 const invalidate=()=>{revision.current++;setResult(null);};
 function settings(value:Values):NacosSettings{return{driver:'nacos',endpoint:value.endpoint.trim().replace(/\/+$/,''),authentication:value.authentication,namespace:value.namespace?.trim()??''};}
 const changed=!!profile&&!!values?.endpoint&&!credentialTargetUnchanged(profile.settings,settings(values));
 function credential(value:Values){return value.authentication==='none'?{action:'remove'}:mode==='replace'?{action:'replaceNacos',username:value.username,password:value.password}:{action:mode};}
 async function save(){
  if(occupied.current)return;occupied.current=true;setBusy(true);setError('');
  try{const value=await form.validateFields();const next=settings(value);
   if(next.authentication==='password'&&mode==='keep'&&profile&&!credentialTargetUnchanged(profile.settings,next))throw '认证目标已变化，请选择替换凭据。';
   const name=value.name.trim()||new URL(next.endpoint).host;
   const receipt=await invoke<MutationReceipt>('profiles_save',{draft:{id:profile?.id??null,expectedRevision:profile?.revision??null,name,settings:next},credential:credential(value)});
   form.setFieldsValue({username:'',password:''});if(mounted.current)onSaved(receipt);
  }catch(e){if(mounted.current&&!(e&&typeof e==='object'&&'errorFields'in e))setError(String(e));}
  finally{occupied.current=false;if(mounted.current)setBusy(false);}
 }
 async function testConnection(){
  if(occupied.current)return;occupied.current=true;setBusy(true);setTesting(true);setResult(null);const epoch=revision.current;
  try{const value=await form.validateFields(['endpoint','authentication','namespace','username','password']);if(!mounted.current||epoch!==revision.current)return;const next=settings(value);
   if(next.authentication==='password'&&mode==='keep'&&profile&&!credentialTargetUnchanged(profile.settings,next))throw '认证目标已变化，请替换凭据后测试。';
   await invoke('profiles_test_nacos',{request:{id:profile?.id??null,expectedRevision:profile?.revision??null,settings:next,credential:credential(value)}});
   if(mounted.current&&epoch===revision.current)setResult({kind:'success',text:'测试连接成功，当前配置尚未保存'});
  }catch(e){if(mounted.current&&epoch===revision.current&&!(e&&typeof e==='object'&&'errorFields'in e))setResult({kind:'error',text:String(e)});}
  finally{occupied.current=false;if(mounted.current){setBusy(false);setTesting(false);}}
 }
 function cancel(){if(occupied.current)return;form.setFieldsValue({username:'',password:''});onClose();}
 return <Modal open width={640} className="connection-dialog nacos-dialog" title={`${profile?'编辑':'新建'} Nacos 连接`} onCancel={cancel} closable={!busy} keyboard={!busy} maskClosable={!busy} footer={<Space><Button loading={testing} disabled={busy&&!testing} onClick={()=>void testConnection()}>测试连接</Button><Button disabled={busy} onClick={cancel}>取消</Button><Button type="primary" loading={busy&&!testing} disabled={testing} onClick={()=>void save()}>保存连接</Button></Space>}>
 {error&&<Alert type="error" title={error}/>}{result&&<Typography.Paragraph data-testid="connection-test-result" type={result.kind==='success'?'success':'danger'}>{result.text}</Typography.Paragraph>}
 <Form form={form} layout="vertical" disabled={busy} onValuesChange={invalidate}>
 <Form.Item name="name" label="连接名称（可选）" rules={[{max:80}]}><Input autoFocus autoComplete="off"/></Form.Item>
 <Form.Item name="endpoint" label="Nacos HTTP API 地址" rules={[{required:true,max:2048}]}><Input autoComplete="off" placeholder="http://localhost:8848/nacos"/></Form.Item>
 <Space align="start"><Form.Item name="authentication" label="认证方式"><Select style={{width:200}} options={[{value:'password',label:'账号密码'},{value:'none',label:'未启用认证'}]}/></Form.Item><Form.Item name="namespace" label="默认命名空间 ID" rules={[{max:256}]}><Input placeholder="空表示 public" autoComplete="off"/></Form.Item></Space>
 {values?.authentication!=='none'&&<>{profile&&<Form.Item label="保存的凭据"><Select aria-label="保存的凭据" value={mode} onChange={next=>{invalidate();setMode(next);form.setFieldsValue({username:'',password:''});}} options={[...(profile.credentialSaved?[{value:'keep',label:'保留已保存凭据'}]:[]),{value:'replace',label:'替换账号密码'},...(profile.credentialSaved?[{value:'remove',label:'移除保存的凭据'}]:[])]}/></Form.Item>}
 {mode==='keep'&&changed&&<Alert type="warning" title="认证目标已变化，请替换凭据。"/>}
 {mode==='replace'&&<><Form.Item name="username" label="账号" rules={[{required:true,max:256}]}><Input autoComplete="off"/></Form.Item><Form.Item name="password" label="密码" rules={[{required:true,max:4096}]}><Input.Password autoComplete="new-password"/></Form.Item></>}
 </>}
 <Typography.Text type="secondary">支持 Nacos 2.x。测试不保存；保存凭据使用系统凭据服务。配置与服务写操作在工作区另行确认并核验结果。</Typography.Text>
 </Form></Modal>;
}
