export type Metadata = { id: string; kind: 'ssh'|'db'|'k8s'; name: string; host?: string; port?: number; username?: string; database?: string; engine?: string; path?: string };
export function formatEndpoint(username: string, host: string, port: number): string {
 const address=host.includes(':')&&!host.startsWith('[')?`[${host}]`:host;
 return `${username}@${address}:${port}`;
}
export function connectionState(status: string): { color: string; label: string } {
 if(status==='connected')return {color:'green',label:'已连接'};
 if(status==='connecting')return {color:'blue',label:'连接中'};
 if(status==='disconnected')return {color:'default',label:'已断开'};
 return {color:'red',label:'连接异常'};
}
export function metadata(value: Record<string, unknown>): Metadata {
 const result: Record<string,unknown> = {};
 for (const key of ['id','kind','name','host','port','username','database','engine','path']) {
  if (typeof value[key] === 'string' || (key === 'port' && typeof value[key] === 'number')) result[key] = value[key];
 }
 return result as Metadata;
}
export class OutputGate {
 private generation = 0;
 private chunks: Uint8Array[] = [];
 private bytes = 0;
 private maxBytes: number;
 constructor(maxBytes = 512 * 1024) { this.maxBytes = maxBytes; }
 next() { this.chunks = []; this.bytes = 0; return ++this.generation; }
 push(generation: number, bytes: Uint8Array) {
  if (generation !== this.generation || this.bytes + bytes.length > this.maxBytes) return false;
  this.chunks.push(bytes); this.bytes += bytes.length; return true;
 }
 drain() { const data = new Uint8Array(this.bytes); let offset = 0; for (const chunk of this.chunks) { data.set(chunk,offset); offset += chunk.length; } this.chunks = []; this.bytes = 0; return data; }
}
export function saveMetadata(item: Metadata) {
 const key = 'aic-ops.connections.v1'; let saved: Metadata[] = [];
 try { const value:unknown=JSON.parse(localStorage.getItem(key) || '[]');saved=Array.isArray(value)?value.filter(v=>v&&typeof v==='object').map(metadata):[]; } catch { /* Ignore invalid local metadata. */ }
 localStorage.setItem(key, JSON.stringify([...saved.filter(v => v.id !== item.id), metadata(item)]));
}
export function loadMetadata(kind: Metadata['kind']): Metadata[] {
 try { const list: unknown = JSON.parse(localStorage.getItem('aic-ops.connections.v1') || '[]'); return Array.isArray(list) ? list.filter(v => v?.kind === kind).map(metadata) : []; } catch { return []; }
}
