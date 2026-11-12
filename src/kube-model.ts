export const RESOURCE_KINDS = ['pods', 'deployments', 'statefulsets', 'daemonsets', 'jobs', 'cronjobs', 'services', 'configmaps', 'secrets', 'persistentvolumeclaims', 'persistentvolumes', 'events'] as const;
export type ResourceKind = typeof RESOURCE_KINDS[number];
export interface KubeScope {sessionId:string;generation:number}
export interface KubeSelection extends KubeScope { generation: number; context: string; namespace: string; contexts: string[]; }
export interface KubeRow { name: string; namespace: string; kind: string; status: string; created: string; containers: string[]; }
export interface KubePage extends KubeScope { generation: number; items: KubeRow[]; continueToken: string; resourceVersion:string; }
export interface KubeChange { event:'ADDED'|'MODIFIED'|'DELETED'; row:KubeRow; }
export interface KubeWatch extends KubeScope { generation:number;resourceVersion:string;changes:KubeChange[];expired:boolean; }
export function applyWatchChanges(rows:KubeRow[],changes:KubeChange[],limit=100):{items:KubeRow[];refresh:boolean} {
  const items=new Map(rows.map(row=>[`${row.namespace}/${row.name}`,row]));
  let refresh=false;
  for(const {event,row} of changes) {
    const key=`${row.namespace}/${row.name}`;
    if(event==='DELETED') items.delete(key);
    else if(items.has(key)||items.size<limit) items.set(key,row);
    else refresh=true;
  }
  return {items:[...items.values()],refresh};
}
export interface KubeDetail extends KubeScope { generation: number; yaml: string; containers: string[]; }
export interface KubeLogPoll extends KubeScope { generation: number; logId: string; text: string; droppedBytes: number; done: boolean; error: string | null; }
export function pageLimit(value: number): number { return Number.isFinite(value) ? Math.max(1, Math.min(200, Math.floor(value))) : 100; }
export function acceptGeneration(expected: number, received: number): boolean { return expected === received; }
export function appendLog(previous: string, next: string, limit = 262144): { text: string; dropped: number } {
  const chars = Array.from(previous + next);
  const dropped = Math.max(0, chars.length - Math.max(1, limit));
  return {text: chars.slice(dropped).join(''), dropped};
}
export function searchRows<T extends {name: string; namespace: string; status: string}>(rows: T[], query: string): T[] {
  const term = query.trim().toLocaleLowerCase();
  return rows.filter(row => `${row.name} ${row.namespace} ${row.status}`.toLocaleLowerCase().includes(term));
}

export function acceptKubeResult(expected:KubeScope|null,received:KubeScope):boolean{return expected!==null&&expected.sessionId===received.sessionId&&expected.generation===received.generation;}

export function statusColor(status:string):string {
 const value=status.trim().toLowerCase();
 if(['running','succeeded','completed','bound','ready','available'].includes(value))return 'green';
 if(['pending','terminating','containercreating'].includes(value))return 'gold';
 if(['failed','error','crashloopbackoff','imagepullbackoff','errimagepull'].includes(value))return 'red';
 return 'default';
}
export function namespaceColor(namespace:string):string {
 if(!namespace)return 'default';let hash=2166136261;for(const char of namespace)hash=Math.imul(hash^char.charCodeAt(0),16777619)>>>0;
 return ['blue','geekblue','purple','cyan','magenta','volcano','gold','green','lime','orange'][hash%10];
}
