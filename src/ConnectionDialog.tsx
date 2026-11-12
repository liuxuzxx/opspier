import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {Alert,Button,Form,Input,InputNumber,Modal,Select,Space,Typography} from 'antd';
import {initialConnectionValues} from './connection-draft';
import {credentialTargetUnchanged,type Profile,type ProfileSettings,type MutationReceipt} from './profiles-model';
type Driver='ssh'|'mysql'|'doris';
type Values={name:string;host:string;port:number;username:string;authentication:'password'|'key';privateKeyPath:string|null;database:string;tls:'auto'|'verifyIdentity'|'required'|'disabled';password:string};
export default function ConnectionDialog({profile,driver,onClose,onSaved}:{profile:Profile|null;driver:Driver;onClose:()=>void;onSaved:(receipt:MutationReceipt)=>void}){
 const [form]=Form.useForm<Values>();const [modal,modalHolder]=Modal.useModal();
 const [testing,setTesting]=useState(false);const [testResult,setTestResult]=useState<{kind:'success'|'error'|'info';text:string}|null>(null);const testRevision=useRef(0);
 function invalidateTest(){testRevision.current++;setTestResult(null);}const [mode,setMode]=useState<'keep'|'replace'|'remove'>(profile?.credentialSaved?'keep':'replace');
 const [busy,setBusy]=useState(false);const [error,setError]=useState('');const saving=useRef(false);const mounted=useRef(true);
 const values=Form.useWatch([],form) as Values|undefined;
 useEffect(()=>{form.setFieldsValue({...initialConnectionValues(driver,profile&&profile.settings.driver!=='kubernetes'&&profile.settings.driver!=='minio'?profile.settings:null),name:profile?.name??'',password:''});return()=>{mounted.current=false;form.setFieldValue('password','');};},[]);
 function settings(value:Values):Exclude<ProfileSettings,{driver:'kubernetes'|'minio'}>{
  const common={host:value.host.trim(),port:value.port,username:value.username.trim()};
  return driver==='ssh'?{driver,...common,authentication:value.authentication,privateKeyPath:value.authentication==='key'?value.privateKeyPath:null}:{driver,...common,database:value.database??'',tls:value.tls??form.getFieldValue('tls')??(driver==='mysql'&&!profile?'auto':'verifyIdentity')};
 }
 const targetChanged=!!profile&&!!values?.host&&!credentialTargetUnchanged(profile.settings,settings(values));
 async function save(){
  if(saving.current)return;saving.current=true;setBusy(true);setError('');
  try{
   const value=await form.validateFields();const next=settings(value);
   if(mode==='keep'&&profile&&!credentialTargetUnchanged(profile.settings,next))throw '认证目标已变化，请选择替换凭据。';
   const name=(value.name??'').trim()||(driver==='doris'||driver==='mysql'&&!profile?`${next.host}:${next.port}`.slice(0,80):'');
   const receipt=await invoke<MutationReceipt>('profiles_save',{draft:{id:profile?.id??null,expectedRevision:profile?.revision??null,name,settings:next},credential:mode==='replace'?{action:'replacePassword',value:value.password??''}:{action:mode}});
   form.setFieldValue('password','');if(mounted.current)onSaved(receipt);
  }catch(e){if(mounted.current&&!(e&&typeof e==='object'&&'errorFields'in e))setError(String(e));}
  finally{saving.current=false;if(mounted.current)setBusy(false);}
 }
 async function testConnection(){
  if((profile&&driver==='ssh')||saving.current)return;saving.current=true;setBusy(true);setTesting(true);setTestResult(null);const revision=testRevision.current;
  try{
   const fields=driver==='ssh'?['host','port','username','authentication','password',...(form.getFieldValue('authentication')==='key'?['privateKeyPath']:[])]:['host','port','username','password','database',...(driver==='mysql'&&profile?['tls']:[])];
   const value=await form.validateFields(fields);if(!mounted.current||revision!==testRevision.current)return;
   const next=settings(value);
   if(profile&&next.driver!=='ssh'){
    if(mode==='keep'&&!credentialTargetUnchanged(profile.settings,next))throw '认证目标已变化，请选择替换凭据后测试。';
    await invoke('profiles_test_db',{request:{id:profile.id,expectedRevision:profile.revision,settings:next,credential:mode==='replace'?{action:'replacePassword',value:value.password??''}:{action:mode}}});
   }else if(next.driver==='ssh'){
    const target={endpoint:{host:next.host,port:next.port,username:next.username},authentication:next.authentication,privateKeyPath:next.privateKeyPath};
    const probe=await invoke<{token:string;host:string;port:number;fingerprint:string;trusted:boolean}>('ssh_test_probe',{target});
    if(!mounted.current||revision!==testRevision.current)return;
    let trust=probe.trusted;
    if(!trust)trust=await new Promise<boolean>(resolve=>modal.confirm({title:'核验测试主机指纹',content:<Space orientation="vertical"><Typography.Text>{probe.host}:{probe.port}</Typography.Text><Typography.Text code copyable>{probe.fingerprint}</Typography.Text><Typography.Text>请通过可信渠道比对指纹。本次仅测试认证，不保存连接。</Typography.Text></Space>,okText:'已核验，继续测试',cancelText:'取消',onOk:()=>resolve(true),onCancel:()=>resolve(false)}));
    if(!trust){if(mounted.current&&revision===testRevision.current)setTestResult({kind:'info',text:'已取消测试'});return;}
    if(!mounted.current||revision!==testRevision.current)return;
    await invoke('ssh_test_authenticate',{request:{target,token:probe.token,trustHost:trust,password:next.authentication==='password'?(value.password??''):null,passphrase:next.authentication==='key'?(value.password??''):null}});
   }else await invoke('db_test_connection',{config:{host:next.host,port:next.port,username:next.username,database:next.database,tls:next.tls,kind:next.driver,name:'',password:value.password??''}});
   if(mounted.current&&revision===testRevision.current)setTestResult({kind:'success',text:profile?'测试连接成功，本次修改尚未保存':'测试连接成功，尚未保存'});
  }catch(e){if(mounted.current&&revision===testRevision.current&&!(e&&typeof e==='object'&&'errorFields'in e))setTestResult({kind:'error',text:String(e)});}
  finally{saving.current=false;if(mounted.current){setBusy(false);setTesting(false);}}
 }
 function cancel(){if(saving.current)return;form.setFieldValue('password','');onClose();}
 return <Modal open className="connection-dialog" title={`${profile?'编辑':'新建'} ${driver==='ssh'?'SSH':driver==='mysql'?'MySQL':'Apache Doris'} 连接`} width={620} onCancel={cancel} closable={!busy} maskClosable={!busy} keyboard={!busy} footer={<Space>{(!profile||driver!=='ssh')&&<Button onClick={()=>void testConnection()} loading={testing} disabled={busy&&!testing}>测试连接</Button>}<Button onClick={cancel} disabled={busy}>取消</Button><Button type="primary" onClick={()=>void save()} loading={busy&&!testing} disabled={testing}>保存连接</Button></Space>}>
 {modalHolder}{testResult&&<Typography.Paragraph data-testid="connection-test-result" type={testResult.kind==='error'?'danger':testResult.kind==='success'?'success':'secondary'} style={{marginBottom:8}}>{testResult.text}</Typography.Paragraph>}
 {error&&<Alert type="error" title={error} style={{marginBottom:16}}/>}
 <Form form={form} layout="vertical" disabled={busy} onValuesChange={invalidateTest}>
 <Form.Item name="name" label={driver==='doris'||driver==='mysql'&&!profile?'连接名称（可选）':'连接名称'} rules={[{required:driver!=='doris'&&(driver!=='mysql'||!!profile),whitespace:true,max:80}]}><Input autoFocus autoComplete="off" placeholder={driver==='doris'||driver==='mysql'&&!profile?'留空按主机和端口命名':undefined}/></Form.Item>
 <Space align="start"><Form.Item name="host" label="主机" rules={[{required:true,max:253}]}><Input placeholder="域名或 IP" autoComplete="off"/></Form.Item><Form.Item name="port" label="端口" rules={[{required:true}]}><InputNumber min={1} max={65535}/></Form.Item></Space>
 <Form.Item name="username" label={driver==='ssh'?'用户名':'只读用户名'} rules={[{required:true,max:128}]}><Input autoComplete="off"/></Form.Item>
 {driver==='ssh'?<><Form.Item name="authentication" label="认证方式"><Select options={[{value:'password',label:'密码'},{value:'key',label:'本机私钥文件'}]}/></Form.Item>{values?.authentication==='key'&&<Form.Item name="privateKeyPath" label="本机私钥绝对路径" rules={[{required:true}]}><Input autoComplete="off" placeholder="/absolute/path/to/key"/></Form.Item>}</>:<><Form.Item name="database" label="默认数据库（可选）"><Input maxLength={128}/></Form.Item>{driver==='mysql'&&profile&&<Form.Item name="tls" label="TLS 策略"><Select options={[{value:'auto',label:'自动协商 TLS'},{value:'verifyIdentity',label:'验证证书与主机名（系统 CA）'},{value:'required',label:'强制加密，不验证证书身份'},{value:'disabled',label:'关闭 TLS（明文，仅可信隔离网络）'}]}/></Form.Item>}</>}
 {profile&&<Form.Item label="保存的凭据"><Select aria-label="保存的凭据" value={mode} onChange={value=>{invalidateTest();form.setFieldValue('password','');setMode(value);}} options={[...(profile?.credentialSaved?[{value:'keep',label:'保留已保存凭据'}]:[]),{value:'replace',label:profile?.credentialSaved?'替换凭据':'保存新凭据'},...(profile?.credentialSaved?[{value:'remove',label:'移除已保存凭据'}]:[])]}/></Form.Item>}
 {mode==='keep'&&targetChanged&&<Alert type="warning" title="认证目标已变化，请选择替换凭据。"/>}
 {mode==='replace'&&<Form.Item name="password" label={driver==='ssh'&&values?.authentication==='key'?'私钥口令（未加密可留空）':'密码（空密码可留空）'}><Input.Password autoComplete="new-password"/></Form.Item>}
 <Typography.Text type="secondary">凭据保存到系统凭据服务；已保存内容不会回填。保存配置后，在左侧打开连接。</Typography.Text>
 </Form></Modal>;
}
