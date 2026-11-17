/** Keeps asynchronous results attached to the target the person actually confirmed. */
export class NacosWorkspace {
 scope='';private epoch=0;private closed=false;private occupied=false;
 private changed:(busy:boolean)=>void;
 constructor(changed:(busy:boolean)=>void=()=>{}){this.changed=changed;}
 get locked(){return this.closed||this.occupied;}
 private setBusy(value:boolean){this.occupied=value;if(!this.closed)this.changed(value);}
 async transition(scope:string,confirm:()=>Promise<boolean>=async()=>true){
  if(this.locked)return false;this.setBusy(true);
  try{if(!await confirm()||this.closed)return false;this.scope=scope;this.epoch++;return true;}
  finally{this.setBusy(false);}
 }
 async query<T>(work:()=>Promise<T>):Promise<T|undefined>{
  if(this.locked)return undefined;const epoch=this.epoch;
  try{const value=await work();return !this.closed&&epoch===this.epoch?value:undefined;}
  catch(error){if(!this.closed&&epoch===this.epoch)throw error;return undefined;}
 }
 async mutate<T>(confirm:()=>Promise<boolean>,work:()=>Promise<T>):Promise<T|undefined>{
  if(this.locked)return undefined;this.setBusy(true);const epoch=++this.epoch;
  try{if(!await confirm()||this.closed||epoch!==this.epoch)return undefined;const value=await work();return !this.closed&&epoch===this.epoch?value:undefined;}
  finally{this.setBusy(false);}
 }
 dispose(){this.closed=true;this.epoch++;}
}
