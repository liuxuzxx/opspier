import type {KubeRow,KubeScope} from './kube-model';
export interface BrowserScope extends KubeScope {context:string;catalogRevision:number;resourceId:string;namespace:string;search:string;cursor:string;page:number}
export interface BrowserPage extends KubeScope {requestId:number;catalogRevision:number;scopeRevision:number;resourceId:string;namespace:string;search:string;items:KubeRow[];continueToken:string;resourceVersion:string;scanned:number;pages:number;bytes:number;truncated:boolean;reason:string|null}
export interface BrowserSnapshot {scope:BrowserScope;data:BrowserPage;localPage:number}
export interface BrowserState {snapshot:BrowserSnapshot|null;pending:BrowserScope|null;interactive:boolean;error:string}
export function browserScopeKey(scope:BrowserScope):string{return JSON.stringify([scope.sessionId,scope.generation,scope.context,scope.catalogRevision,scope.resourceId,scope.namespace,scope.search,scope.cursor,scope.page]);}
export class KubeBrowser {
 state:BrowserState={snapshot:null,pending:null,interactive:false,error:''};
 private nextId=0;private disposed=false;private active:{id:number;scope:BrowserScope}|null=null;
 private transport:(scope:BrowserScope,id:number)=>Promise<BrowserPage>;private cancel:(scope:BrowserScope,id:number)=>Promise<unknown>;private notify:(state:BrowserState)=>void;
 constructor(transport:(scope:BrowserScope,id:number)=>Promise<BrowserPage>,cancel:(scope:BrowserScope,id:number)=>Promise<unknown>,notify:(state:BrowserState)=>void){this.transport=transport;this.cancel=cancel;this.notify=notify;}
 private publish(state:BrowserState){this.state=state;if(!this.disposed)this.notify(state);}
 invalidate(){
  if(this.disposed)return;this.nextId++;if(this.active)void this.cancel(this.active.scope,this.active.id).catch(()=>{});this.active=null;
  this.publish({...this.state,pending:null,interactive:false});
 }
 setLocalPage(page:number){
  const snapshot=this.state.snapshot;
  if(!snapshot?.scope.search||!this.state.interactive||!Number.isInteger(page)||page<0||page>Math.max(0,Math.ceil(snapshot.data.items.length/100)-1))return;
  this.publish({...this.state,snapshot:{...snapshot,localPage:page}});
 }
 async run(scope:BrowserScope,background=false):Promise<void>{
  if(this.disposed||(background&&this.active))return;
  const id=++this.nextId;const previous=this.active;this.active={id,scope};
  if(previous)void this.cancel(previous.scope,previous.id).catch(()=>{});
  const same=this.state.snapshot&&browserScopeKey(this.state.snapshot.scope)===browserScopeKey(scope);
  this.publish({...this.state,pending:scope,interactive:!!same&&this.state.interactive,error:''});
  try{
   const reply=await this.transport(scope,id);
   if(this.disposed||id!==this.nextId)return;
   if(reply.sessionId!==scope.sessionId||reply.generation!==scope.generation||reply.catalogRevision!==scope.catalogRevision||reply.requestId!==id||reply.resourceId!==scope.resourceId||reply.namespace!==scope.namespace||reply.search!==scope.search.trim())throw '查询返回的范围已变化，请重新加载。';
   const previous=this.state.snapshot;
   const same=previous&&browserScopeKey(previous.scope)===browserScopeKey(scope);
   const localPage=same?Math.min(previous.localPage,Math.max(0,Math.ceil(reply.items.length/100)-1)):0;
   this.publish({snapshot:{scope:{...scope,search:reply.search,namespace:reply.namespace},data:reply,localPage},pending:scope,interactive:true,error:''});
  }catch(error){if(!this.disposed&&id===this.nextId)this.publish({...this.state,error:String(error)});}
  finally{if(!this.disposed&&id===this.nextId){this.active=null;this.publish({...this.state,pending:null});}}
 }
 dispose(){if(this.disposed)return;this.disposed=true;this.nextId++;if(this.active)void this.cancel(this.active.scope,this.active.id).catch(()=>{});this.active=null;}
}
