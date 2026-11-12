export type DraftDriver='ssh'|'mysql'|'doris';
type PublicFields={port:number;host:string;username:string;authentication:'password'|'key';privateKeyPath:string|null;database:string;tls:'auto'|'verifyIdentity'|'required'|'disabled'};
export function initialConnectionValues(driver:DraftDriver,existing:Partial<PublicFields>|null){
 return {port:driver==='ssh'?22:driver==='doris'?9030:3306,authentication:'password' as const,database:'',tls:driver==='mysql'?'auto' as const:'verifyIdentity' as const,...(existing??{})};
}
