import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Alert, Button, Card, Col, Input, InputNumber, Row, Space, Table, Tabs, Tag, Tooltip, Tree, Typography } from 'antd';
import type { DataNode } from 'antd/es/tree';
import { QueryGeneration } from './db-model';
import type { DbPage, DbSession } from './db-model';

interface Catalog { names: string[]; truncated: boolean }
interface Inspection { columns: DbPage; indexes: DbPage }
function ResultTable({ page, onPage }: { page: DbPage; onPage?: (offset: number, limit: number) => void }) {
  return <Table size="small" virtual scroll={{ x: Math.max(800, page.columns.length * 180), y: 350 }} rowKey="__key"
    columns={page.columns.map((column, i) => ({ title: <span title={column.dataType}>{column.name}</span>, dataIndex: String(i), key: String(i), width: 180, ellipsis: true, render: (v: string | null) => v === null ? <Typography.Text type="secondary">NULL</Typography.Text> : <span title={v}>{v}</span> }))}
    dataSource={page.rows.map((row, i) => Object.assign({ __key: String(page.offset + i) }, Object.fromEntries(row.map((value, n) => [String(n), value]))))}
    pagination={onPage ? { current: Math.floor(page.offset / page.limit) + 1, pageSize: page.limit, total: page.total, showSizeChanger: true, pageSizeOptions: [25, 50, 100, 200], onChange: (p, size) => onPage((p - 1) * size, size), showTotal: total => `已缓存 ${total} 行` } : false} />;
}
export default function DbPanel({initialSession,endpoint,onClose}:{initialSession:DbSession;endpoint:string;onClose:()=>void}) {
  const [session,setSession]=useState<DbSession|undefined>(initialSession);
  const [connectedEndpoint,setConnectedEndpoint]=useState(endpoint);
  const [error, setError] = useState('');
  const [sql, setSql] = useState('SELECT 1 AS example');
  const [maxRows, setMaxRows] = useState(1000);
  const [timeout, setTimeoutValue] = useState(30);
  const [tree, setTree] = useState<DataNode[]>([]);
  const [catalogTruncated, setCatalogTruncated] = useState(false);
  const [inspection, setInspection] = useState<Inspection>();
  const [page, setPage] = useState<DbPage>();
  const [running, setRunning] = useState(false);
  const generation = useRef(new QueryGeneration());
  const inspectionGeneration=useRef(new QueryGeneration());
  const sqlRevision=useRef(0);
  const [inspecting,setInspecting]=useState(false);
  const sessionRef = useRef<string | undefined>(initialSession.id);
  const queryRef = useRef<string | undefined>(undefined);
  const cancelRequested = useRef(false);
  const polling = useRef(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false; generation.current.invalidate();inspectionGeneration.current.invalidate();setInspecting(false);
      const id = sessionRef.current;
      if (id) void invoke('db_disconnect', { sessionId: id }).catch(() => {});
    };
  }, []);
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => {
      if (polling.current || !queryRef.current) return;
      polling.current = true;
      const id = queryRef.current;
      void invoke<DbPage>('db_query_page', { queryId: id, offset: 0, limit: 50 }).then(result => {
        if (!mounted.current || queryRef.current !== id) return;
        setPage(result);
        if (result.status !== 'running') { setRunning(false); if (result.error) setError(result.error); }
      }).catch(e => { if (mounted.current && queryRef.current === id) { setError(String(e)); setRunning(false); } }).finally(() => { polling.current = false; });
    }, 350);
    return () => window.clearInterval(timer);
  }, [running]);

  const loadCatalog = async (id: string) => {
    const result = await invoke<Catalog>('db_databases', { sessionId: id });
    if (sessionRef.current !== id) return;
    setTree(result.names.map(name => ({ title: name, key: JSON.stringify([name]), isLeaf: false })));
    setCatalogTruncated(result.truncated);
  };
  const disconnect = async () => {
    generation.current.invalidate();inspectionGeneration.current.invalidate();setInspecting(false);
    const id = sessionRef.current; sessionRef.current = undefined; queryRef.current = undefined;
    setSession(undefined); setConnectedEndpoint(''); setPage(undefined); setTree([]); setInspection(undefined); setRunning(false);
    if (id) await invoke('db_disconnect', { sessionId: id });
    onClose();
  };
  useEffect(()=>{void loadCatalog(initialSession.id).catch(e=>setError(String(e)));},[]);
  const loadTables = async (node: DataNode) => {
    const id = sessionRef.current; if (!id) return;
    const [database] = JSON.parse(String(node.key)) as string[];
    const result = await invoke<Catalog>('db_tables', { sessionId: id, database });
    if (id !== sessionRef.current) return;
    setTree(old => old.map(n => n.key === node.key ? { ...n, children: result.names.map(name => ({ title: name, key: JSON.stringify([database, name]), isLeaf: true })) } : n));
    if (result.truncated) setCatalogTruncated(true);
  };
  const inspect = async (key: string) => {
    const id = sessionRef.current; const [database, table] = JSON.parse(key) as string[];
    if (!id || !table) return;
    const request=inspectionGeneration.current.next();const revision=sqlRevision.current;setInspecting(true);
    try {
      const result = await invoke<Inspection>('db_describe', { sessionId: id, database, table });
      if(id!==sessionRef.current||!inspectionGeneration.current.isCurrent(request))return;
      setInspection(result);
      if(revision===sqlRevision.current)setSql(`SELECT * FROM \`${database.replaceAll('`', '``')}\`.\`${table.replaceAll('`', '``')}\``);
    } catch (e) { if(id===sessionRef.current&&inspectionGeneration.current.isCurrent(request))setError(String(e)); }
    finally {if(inspectionGeneration.current.isCurrent(request))setInspecting(false);}
  };
  const run = async (explain: boolean) => {
    if (!sessionRef.current) return;
    const token = generation.current.next(); const id = sessionRef.current;
    const previousQuery = queryRef.current; queryRef.current = undefined; cancelRequested.current = false;
    setError(''); setRunning(true);
    try {
      if (previousQuery) await invoke('db_forget_query', { queryId: previousQuery });
      const queryId = await invoke<string>('db_query', { sessionId: id, sql, maxRows, timeoutSecs: timeout, explain });
      if (!mounted.current || !generation.current.isCurrent(token) || id !== sessionRef.current) { void invoke('db_forget_query', { queryId }); return; }
      queryRef.current = queryId;
      if (cancelRequested.current) await invoke('db_cancel', { queryId });
      setPage(await invoke<DbPage>('db_query_page', { queryId, offset: 0, limit: 50 }));
    } catch (e) { if (generation.current.isCurrent(token)) { setError(String(e)); setRunning(false); } }
  };
  const cancel = async () => {
    cancelRequested.current = true;
    if (queryRef.current) { await invoke('db_cancel', { queryId: queryRef.current }); setRunning(false); setPage(await invoke<DbPage>('db_query_page', { queryId: queryRef.current, offset: 0, limit: 50 })); }
  };
  const exportCsv = async () => {
    if (!queryRef.current) return;
    try {
      const data = await invoke<string>('db_export_csv', { queryId: queryRef.current });
      const url = URL.createObjectURL(new Blob(['\uFEFF', data], { type: 'text/csv;charset=utf-8' }));
      const a = document.createElement('a'); a.href = url; a.download = 'ops-query.csv'; a.click(); window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (e) { setError(String(e)); }
  };
  return <Space className="compact-workspace" direction="vertical" size={8} style={{ width: '100%' }}>
    {error && <Alert type="error" closable onClose={() => setError('')} title={error} />}
    {session&&<div className="workspace-toolbar"><Space wrap size={8}><Tag color="green">已连接</Tag><Typography.Text>{session.kind} · {session.version}</Typography.Text><Typography.Text data-testid="db-endpoint">{connectedEndpoint}</Typography.Text>
      <Tooltip title="仅允许单条只读查询；缓存最多10000行/8MiB，请使用只读账号。"><Tag color="blue">只读</Tag></Tooltip>
      <Tooltip title={session.notes.join('；')}><Tag color={session.notes.some(n=>n.includes('TLS 已关闭')||n.includes('不验证证书'))?'orange':'default'}>{session.notes.some(n=>n.includes('TLS 已关闭'))?'TLS关闭':session.notes.some(n=>n.includes('不验证证书'))?'TLS未校验':'TLS已校验'}</Tag></Tooltip>
      <Button size="small" onClick={()=>void loadCatalog(session.id).catch(e=>setError(String(e)))}>刷新库表</Button><Button size="small" onClick={()=>void disconnect().catch(e=>setError(String(e)))}>断开</Button>
    </Space></div>}
    {session && <Row gutter={16}>
      <Col xs={24} lg={6}><Card size="small" title="库 / 表">{catalogTruncated && <Alert type="warning" title="目录已达到 2000 项或内存上限，显示部分条目" />}<Tree disabled={inspecting} treeData={tree} loadData={node => loadTables(node).catch(e => { setError(String(e)); })} onSelect={keys => { if (keys[0]) void inspect(String(keys[0])); }} style={{ maxHeight: 700, overflow: 'auto' }} /></Card></Col>
      <Col xs={24} lg={18}><Space direction="vertical" style={{ width: '100%' }}>
        <Card size="small" title="SQL"><Input.TextArea value={sql} onChange={e => {sqlRevision.current++;setSql(e.target.value);}} autoSize={{ minRows: 3, maxRows: 10 }} spellCheck={false} maxLength={65536} style={{ fontFamily: 'monospace' }} /><Space wrap style={{ marginTop: 8 }}>
          <Button type="primary" disabled={running} onClick={() => void run(false)}>执行只读查询</Button><Button disabled={running} onClick={() => void run(true)}>执行计划</Button><Button danger disabled={!running} onClick={() => void cancel().catch(e => setError(String(e)))}>取消</Button>
          <span>行上限</span><InputNumber min={1} max={10000} value={maxRows} onChange={v => setMaxRows(v ?? 1000)} /><span>超时（秒）</span><InputNumber min={1} max={120} value={timeout} onChange={v => setTimeoutValue(v ?? 30)} />
        </Space></Card>
        {page && <Card size="small" title={<Space><Tag color={page.status === 'running' ? 'processing' : 'default'}>{page.status}</Tag>{page.total} 行 · {page.elapsedMs} ms{page.truncated && <Tag color="orange">已截断</Tag>}</Space>} extra={<Button disabled={running || !page.columns.length} onClick={() => void exportCsv()}>导出缓存 CSV</Button>}><ResultTable page={page} onPage={(offset, limit) => { const id = queryRef.current; if (id) void invoke<DbPage>('db_query_page', { queryId: id, offset, limit }).then(p => { if (id === queryRef.current) setPage(p); }).catch(e => setError(String(e))); }} />{page.truncated && <Typography.Text type="warning">只导出当前缓存的有限结果；截断结果并非完整数据集。</Typography.Text>}</Card>}
        {inspection && <Card size="small" title="表结构"><Tabs items={[{ key: 'columns', label: '字段', children: <ResultTable page={inspection.columns} /> }, { key: 'indexes', label: '索引', children: <ResultTable page={inspection.indexes} /> }]} /></Card>}
      </Space></Col>
    </Row>}
  </Space>;
}
