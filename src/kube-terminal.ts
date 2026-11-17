export type TerminalScope={sessionId:string;generation:number;revision:number;terminalId:string};
export type TerminalPoll={data:number[];done:boolean;queuedBytes:number};
type Invoke=(command:string,args:any)=>Promise<any>;
export type TerminalState={status:'closed'|'opening'|'connected'|'exited'|'error';scope:TerminalScope|null;error:string;namespace:string;appliedNamespace:string;updating:boolean};
export class KubeTerminalController{
 private identity:{sessionId:string;generation:number};private invoke:Invoke;private changed:()=>void;private revision=0;private disposed=false;private suspended=false;private syncing=false;private queued:Uint8Array[]=[];private queuedBytes=0;private sending=false;
 state:TerminalState;
 constructor(identity:{sessionId:string;generation:number},namespace:string,invoke:Invoke,changed:()=>void){this.identity={sessionId:identity.sessionId,generation:identity.generation};this.invoke=invoke;this.changed=changed;this.state={status:'closed',scope:null,error:'',namespace,appliedNamespace:namespace,updating:false};}
 private emit(){if(!this.disposed)this.changed();}
 private current(scope:TerminalScope){return !this.disposed&&this.state.scope?.terminalId===scope.terminalId&&this.state.scope.revision===scope.revision;}
 private get canDrainInput(){return !this.disposed&&!this.suspended&&this.state.status==='connected'&&!this.state.updating;}
 get canInput(){return this.canDrainInput&&this.state.namespace===this.state.appliedNamespace;}
 setSuspended(value:boolean){this.suspended=value;this.emit();if(!value){if(this.queued.length)void this.pump();else void this.syncNamespace();}}
 setVisible(_visible:boolean){} // Visibility never changes terminal ownership or its process.
 async open(kubectlPath:string,shellPath:string){
  if(this.disposed||this.state.status==='opening'||this.state.status==='connected')return;
  if(this.state.scope)await this.close();if(this.disposed)return;
  const scope={...this.identity,revision:++this.revision,terminalId:crypto.randomUUID()};const namespace=this.state.namespace;
  this.state={...this.state,status:'opening',scope,error:'',updating:true};this.emit();
  try{
   const result=await this.invoke('terminal_open',{scope,namespace,kubectlPath:kubectlPath||null,shellPath:shellPath||null,cols:100,rows:30});
   if(!this.current(scope)){await this.invoke('terminal_close',{scope}).catch(()=>{});return;}
   if(result.terminalId!==scope.terminalId||result.sessionId!==scope.sessionId||result.generation!==scope.generation||result.revision!==scope.revision)throw Error('终端返回的工作区标识不一致');
   this.state={...this.state,status:'connected',appliedNamespace:namespace,updating:false};this.emit();await this.syncNamespace();
  }catch(error){if(this.current(scope)){this.state={...this.state,status:'error',scope:null,updating:false,error:String(error)};this.emit();void this.invoke('terminal_close',{scope}).catch(()=>{});}}
 }
 async close(){const scope=this.state.scope;this.queued=[];this.queuedBytes=0;this.state={...this.state,status:'closed',scope:null,updating:false,error:''};this.emit();if(scope)try{await this.invoke('terminal_close',{scope});}catch(error){if(!this.disposed&&!this.state.scope){this.state.error=String(error);this.emit();}}}
 async namespace(value:string){this.state.namespace=value;this.emit();await this.syncNamespace();}
 private async syncNamespace(){
  // 已接受的命令片段先完整发出，再更新默认命名空间。
  if(this.syncing||this.suspended||this.sending||this.queued.length||this.state.status!=='connected'||!this.state.scope)return;this.syncing=true;const scope=this.state.scope;
  try{while(this.current(scope)&&this.state.namespace!==this.state.appliedNamespace){const namespace=this.state.namespace;this.state.updating=true;this.emit();await this.invoke('terminal_namespace',{scope,namespace});if(!this.current(scope))return;this.state.appliedNamespace=namespace;this.state.error='';}}
  catch(error){if(this.current(scope))this.state.error=String(error);}
  finally{this.syncing=false;if(this.current(scope)){this.state.updating=false;this.emit();}else if(this.state.scope){void this.syncNamespace();}}
 }
 send(text:string){
  if(!this.canInput)return false;const bytes=new TextEncoder().encode(text);
  if(bytes.length>65536||this.queuedBytes+bytes.length>131072){this.state.error='单次输入上限64KiB；输入队列上限128KiB，请稍后重试';this.emit();return false;}
  this.queued.push(bytes);this.queuedBytes+=bytes.length;void this.pump();return true;
 }
 private async pump(){if(this.sending)return;this.sending=true;const scope=this.state.scope;
  try{while(scope&&this.current(scope)&&this.canDrainInput&&this.queued.length){const bytes=this.queued.shift()!;this.queuedBytes-=bytes.length;await this.invoke('terminal_input',{scope,data:Array.from(bytes)});}}
  catch{if(scope&&this.current(scope)){this.queued=[];this.queuedBytes=0;this.state={...this.state,status:'error',error:'终端输入失败，已停止此终端以避免执行残缺命令，请重新打开。'};this.emit();await this.invoke('terminal_close',{scope}).catch(()=>{});}}
  finally{this.sending=false;if(this.queued.length&&this.canDrainInput){void this.pump();}else{void this.syncNamespace();}}
 }
 async poll():Promise<TerminalPoll|null>{const scope=this.state.scope;if(!scope||this.state.status!=='connected')return null;const result=await this.invoke('terminal_poll',{scope}) as TerminalPoll;if(!this.current(scope))return null;if(result.done){this.state.status='exited';this.emit();}return result;}
 async resize(cols:number,rows:number){const scope=this.state.scope;if(scope&&this.state.status==='connected')await this.invoke('terminal_resize',{scope,cols:Math.max(2,Math.min(500,cols)),rows:Math.max(2,Math.min(300,rows))});}
 dispose(){if(this.disposed)return;const scope=this.state.scope;this.disposed=true;this.queued=[];this.queuedBytes=0;this.state.scope=null;if(scope)void this.invoke('terminal_close',{scope}).catch(()=>{});}
}
