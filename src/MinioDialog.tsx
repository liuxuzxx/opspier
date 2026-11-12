import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {Alert,Button,Form,Input,Modal,Select,Space,Typography} from 'antd';
import {credentialTargetUnchanged,type MinioSettings,type MutationReceipt,type Profile} from './profiles-model';
import {initialMinioValues} from './minio-model';
type Values=ReturnType<typeof initialMinioValues>;
export default function MinioDialog({profile,onClose,onSaved}:{profile:Profile|null;onClose:()=>void;onSaved:(value:MutationReceipt)=>void}){
 const [form]=Form.useForm<Values>();const [mode,setMode]=useState<'keep'|'replace'|'remove'>(profile?.credentialSaved?'keep':'replace');
 const [busy,setBusy]=useState(false);const [testing,setTesting]=useState(false);const [error,setError]=useState('');
 const [testResult,setTestResult]=useState<{kind:'success'|'error';text:string}|null>(null);const saving=useRef(false);const mounted=useRef(true);const testRevision=useRef(0);
 const values=Form.useWatch([],form) as Values|undefined;
 useEffect(()=>{form.setFieldsValue(initialMinioValues(profile?.settings.driver==='minio'?{name:profile.name,settings:profile.settings}:null));return()=>{mounted.current=false;form.setFieldsValue({accessKey:'',secretKey:''});};},[]);
 function invalidateTest(){testRevision.current++;setTestResult(null);}
 function settings(value:Values):MinioSettings{return {driver:'minio',endpoint:value.endpoint.trim().replace(/\/$/,''),region:value.region.trim()||'us-east-1',bucket:value.bucket?.trim()??''};}
 const targetChanged=!!profile&&!!values?.endpoint&&!credentialTargetUnchanged(profile.settings,settings(values));
 async function save(){
  if(saving.current)return;saving.current=true;setBusy(true);setError('');
  try{
   const value=await form.validateFields();const next=settings(value);
   if(mode==='keep'&&profile&&!credentialTargetUnchanged(profile.settings,next))throw '认证目标已变化，请选择替换密钥。';
   const name=value.name.trim()||next.endpoint.replace(/^https?:\/\//,'').slice(0,80);
   const receipt=await invoke<MutationReceipt>('profiles_save',{draft:{id:profile?.id??null,expectedRevision:profile?.revision??null,name,settings:next},credential:mode==='replace'?{action:'replaceMinio',accessKey:value.accessKey,secretKey:value.secretKey}:{action:mode}});
   form.setFieldsValue({accessKey:'',secretKey:''});if(mounted.current)onSaved(receipt);
  }catch(e){if(mounted.current&&!(e&&typeof e==='object'&&'errorFields'in e))setError(String(e));}
  finally{saving.current=false;if(mounted.current)setBusy(false);}
 }
 async function testConnection(){
  if(saving.current)return;saving.current=true;setBusy(true);setTesting(true);setTestResult(null);const revision=testRevision.current;
  try{
   const value=await form.validateFields(['endpoint','region','bucket','accessKey','secretKey']);const next=settings(value);
   if(mode==='keep'&&profile&&!credentialTargetUnchanged(profile.settings,next))throw '认证目标已变化，请选择替换密钥后测试。';
   await invoke('profiles_test_minio',{request:{id:profile?.id??null,expectedRevision:profile?.revision??null,settings:next,credential:mode==='replace'?{action:'replaceMinio',accessKey:value.accessKey,secretKey:value.secretKey}:{action:mode}}});
   if(mounted.current&&revision===testRevision.current)setTestResult({kind:'success',text:'测试连接成功，当前配置尚未保存'});
  }catch(e){if(mounted.current&&revision===testRevision.current&&!(e&&typeof e==='object'&&'errorFields'in e))setTestResult({kind:'error',text:String(e)});}
  finally{saving.current=false;if(mounted.current){setBusy(false);setTesting(false);}}
 }
 function cancel(){if(saving.current)return;form.setFieldsValue({accessKey:'',secretKey:''});onClose();}
 return <Modal open width={620} className="connection-dialog minio-dialog" title={`${profile?'编辑':'新建'} MinIO 连接`} onCancel={cancel} closable={!busy} keyboard={!busy} maskClosable={!busy} footer={<Space><Button loading={testing} disabled={busy&&!testing} onClick={()=>void testConnection()}>测试连接</Button><Button disabled={busy} onClick={cancel}>取消</Button><Button type="primary" loading={busy&&!testing} disabled={testing} onClick={()=>void save()}>保存连接</Button></Space>}>
 {error&&<Alert type="error" title={error} style={{marginBottom:12}}/>}{testResult&&<Typography.Paragraph data-testid="connection-test-result" type={testResult.kind==='success'?'success':'danger'}>{testResult.text}</Typography.Paragraph>}
 <Form form={form} layout="vertical" disabled={busy} onValuesChange={invalidateTest}>
 <Form.Item name="name" label="连接名称（可选）" rules={[{max:80}]}><Input autoFocus autoComplete="off" placeholder="留空按 API 地址命名"/></Form.Item>
 <Form.Item name="endpoint" label="API 地址" rules={[{required:true,max:2048}]}><Input placeholder="http://host:9000 或 https://host:9000" autoComplete="off"/></Form.Item>
 <Space align="start" style={{display:'flex'}}><Form.Item name="region" label="Region" rules={[{required:true,max:128}]}><Input autoComplete="off"/></Form.Item><Form.Item name="bucket" label="默认桶（可选）" rules={[{max:63}]}><Input placeholder="仅有指定桶权限时填写" autoComplete="off"/></Form.Item></Space>
 {profile&&<Form.Item label="保存的密钥"><Select aria-label="保存的密钥" value={mode} onChange={next=>{invalidateTest();form.setFieldsValue({accessKey:'',secretKey:''});setMode(next);}} options={[...(profile.credentialSaved?[{value:'keep',label:'保留已保存密钥'}]:[]),{value:'replace',label:'替换密钥'},...(profile.credentialSaved?[{value:'remove',label:'移除已保存密钥'}]:[])]}/></Form.Item>}
 {mode==='keep'&&targetChanged&&<Alert type="warning" title="认证目标已变化，请选择替换密钥。"/>}
 {mode==='replace'&&<><Form.Item name="accessKey" label="Access Key" rules={[{required:true,max:256}]}><Input.Password autoComplete="new-password"/></Form.Item><Form.Item name="secretKey" label="Secret Key" rules={[{required:true,max:1024}]}><Input.Password autoComplete="new-password"/></Form.Item></>}
 <Typography.Text type="secondary">使用 S3 API 地址，通常为 9000 端口。HTTPS 校验证书；密钥存入系统凭据库，重开连接无需再次输入。</Typography.Text>
 </Form></Modal>;
}
