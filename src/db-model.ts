export type DbKind = 'mysql' | 'doris';
export type DbTls = 'verifyIdentity' | 'required' | 'disabled';
export interface DbMetadata { name: string; host: string; port: number; username: string; database: string; kind: DbKind; tls: DbTls }
export interface DbConnection extends DbMetadata { password: string }
export interface DbColumn { name: string; dataType: string }
export interface DbPage { queryId: string; columns: DbColumn[]; rows: (string | null)[][]; total: number; offset: number; limit: number; truncated: boolean; elapsedMs: number; status: 'running' | 'completed' | 'cancelled' | 'failed'; error: string | null }
export interface DbSession { id: string; kind: DbKind; version: string; readOnlyTransaction: boolean; notes: string[] }
export function boundedPage(offset: number, limit: number, total: number) {
  if (![offset, limit, total].every(Number.isSafeInteger) || offset < 0 || limit < 1 || total < 0) throw new Error('Invalid page');
  limit = Math.min(limit, 200);
  const last = total ? Math.floor((total - 1) / limit) * limit : 0;
  return { offset: Math.min(Math.floor(offset / limit) * limit, last), limit };
}
export function csvCell(value: string | null) {
  let text = value ?? '';
  if (/^[\s\u0000-\u001f]*[=+\-@]/u.test(text)) text = `'${text}`;
  return `"${text.replaceAll('"', '""')}"`;
}
export function metadataOnly(value: DbConnection): DbMetadata {
  return { name: value.name, host: value.host, port: value.port, username: value.username, database: value.database, kind: value.kind, tls: value.tls };
}
export class QueryGeneration {
  private value = 0;
  next() { return ++this.value; }
  isCurrent(value: number) { return value === this.value; }
  invalidate() { ++this.value; }
}
