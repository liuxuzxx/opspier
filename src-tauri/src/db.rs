//! Dedicated, cancellable, read-only MySQL protocol sessions. Secrets never leave memory.
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlparser::{
    ast::{self, Visit, Visitor},
    dialect::MySqlDialect,
    parser::Parser,
    tokenizer::{Token, Tokenizer, Whitespace},
};
use sqlx::{
    Column, ConnectOptions, Connection, Executor, MySqlConnection, Row, TypeInfo, ValueRef,
    mysql::{MySqlConnectOptions, MySqlRow, MySqlSslMode},
};
use std::{
    collections::HashMap,
    ops::ControlFlow,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::task::AbortHandle;
use uuid::Uuid;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ROWS: usize = 10_000;
const MAX_QUERIES: usize = 4;
const REJECTED: &str = "SQL 被拒绝：仅支持经过语法树校验的单条只读查询；不允许写入、锁、文件、变量赋值、执行式注释或未知函数";
const QUERY_ERROR: &str = "数据库查询失败，请核对权限、对象和 SQL（服务端原始错误已隐藏）";

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DbKind {
    Mysql,
    Doris,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DbTls {
    VerifyIdentity,
    Required,
    Disabled,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub database: String,
    pub kind: DbKind,
    pub tls: DbTls,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbSession {
    pub(crate) id: String,
    kind: DbKind,
    version: String,
    read_only_transaction: bool,
    notes: Vec<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbColumn {
    name: String,
    data_type: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbPage {
    query_id: String,
    columns: Vec<DbColumn>,
    rows: Vec<Vec<Option<String>>>,
    total: usize,
    offset: usize,
    limit: usize,
    truncated: bool,
    elapsed_ms: u64,
    status: String,
    error: Option<String>,
}
#[derive(Serialize)]
pub struct DbCatalog {
    names: Vec<String>,
    truncated: bool,
}
#[derive(Serialize)]
pub struct DbInspection {
    columns: DbPage,
    indexes: DbPage,
}
struct QueryRecord {
    columns: Vec<DbColumn>,
    rows: Vec<Vec<Option<String>>>,
    bytes: usize,
    truncated: bool,
    status: &'static str,
    error: Option<String>,
    elapsed_ms: u64,
}
struct QueryState {
    session_id: String,
    started: Instant,
    record: Mutex<QueryRecord>,
    abort: Mutex<Option<AbortHandle>>,
}
impl QueryState {
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            started: Instant::now(),
            record: Mutex::new(QueryRecord {
                columns: vec![],
                rows: vec![],
                bytes: 0,
                truncated: false,
                status: "running",
                error: None,
                elapsed_ms: 0,
            }),
            abort: Mutex::new(None),
        }
    }
    fn finish(&self, result: Result<(), String>) {
        if let Ok(mut record) = self.record.lock() {
            if record.status != "running" {
                return;
            }
            record.elapsed_ms = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            record.status = if result.is_ok() {
                "completed"
            } else {
                "failed"
            };
            record.error = result.err();
        }
    }
    fn cancel(&self) {
        if let Ok(mut record) = self.record.lock() {
            if record.status == "running" {
                record.status = "cancelled";
                record.elapsed_ms = self.started.elapsed().as_millis() as u64;
            }
        }
        if let Ok(mut handle) = self.abort.lock() {
            if let Some(handle) = handle.take() {
                handle.abort();
            }
        }
    }
    fn page(&self, id: String, offset: usize, limit: usize) -> Result<DbPage, String> {
        let record = self.record.lock().map_err(|_| "结果状态不可用")?;
        let (start, end) = page_bounds(offset, limit, record.rows.len())?;
        Ok(DbPage {
            query_id: id,
            columns: record.columns.clone(),
            rows: record.rows[start..end].to_vec(),
            total: record.rows.len(),
            offset: start,
            limit: limit.min(200),
            truncated: record.truncated,
            elapsed_ms: if record.status == "running" {
                self.started.elapsed().as_millis() as u64
            } else {
                record.elapsed_ms
            },
            status: record.status.into(),
            error: record.error.clone(),
        })
    }
}
#[derive(Default)]
struct DbInner {
    sessions: HashMap<String, Arc<DbConfig>>,
    queries: HashMap<String, Arc<QueryState>>,
    metadata: HashMap<String, (String, tokio::sync::watch::Sender<bool>)>,
}
#[derive(Default)]
pub struct DbService {
    inner: Mutex<DbInner>,
}
struct MetadataLease<'a> {
    service: &'a DbService,
    session_id: String,
    id: String,
}
impl Drop for MetadataLease<'_> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.service.inner.lock() {
            if inner
                .metadata
                .get(&self.session_id)
                .is_some_and(|(id, _)| id == &self.id)
            {
                inner.metadata.remove(&self.session_id);
            }
        }
    }
}
impl Drop for DbService {
    fn drop(&mut self) {
        if let Ok(inner) = self.inner.lock() {
            for (_, cancel) in inner.metadata.values() {
                let _ = cancel.send(true);
            }
            for state in inner.queries.values() {
                state.cancel();
            }
        }
    }
}

fn page_bounds(offset: usize, limit: usize, total: usize) -> Result<(usize, usize), String> {
    if limit == 0 {
        return Err("分页大小必须大于零".into());
    }
    let limit = limit.min(200);
    let last = total.saturating_sub(1) / limit * limit;
    let start = (offset / limit * limit).min(last);
    Ok((start, start.saturating_add(limit).min(total)))
}
fn csv_cell(value: Option<&str>) -> String {
    let value = value.unwrap_or("");
    let significant = value.trim_start_matches(|c: char| c.is_whitespace() || c.is_control());
    let prefix = if significant.starts_with(['=', '+', '-', '@']) {
        "'"
    } else {
        ""
    };
    format!("\"{prefix}{}\"", value.replace('"', "\"\""))
}
fn identifier(value: &str) -> Result<String, String> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err("数据库或表名称无效".into());
    }
    Ok(format!("`{}`", value.replace('`', "``")))
}

/// Parse the whole input, then visit every nested query/CTE/expression. Unknown syntax fails closed.
fn validate_read_only(sql: &str, kind: DbKind) -> Result<bool, String> {
    if sql.is_empty() || sql.len() > 65536 {
        return Err(REJECTED.into());
    }
    let dialect = MySqlDialect {};
    let tokens = Tokenizer::new(&dialect, sql)
        .tokenize()
        .map_err(|_| REJECTED)?;
    for token in &tokens {
        match token {
            Token::Assignment | Token::AtSign | Token::Backslash => return Err(REJECTED.into()),
            Token::Whitespace(Whitespace::MultiLineComment(c))
                if c.trim_start().starts_with(['!', '+'])
                    || c.trim_start().to_ascii_uppercase().starts_with("M!") =>
            {
                return Err(REJECTED.into());
            }
            // These modifiers have server-specific meanings even when nested in SELECT.
            Token::Word(w)
                if w.quote_style.is_none()
                    && matches!(
                        w.value.to_ascii_uppercase().as_str(),
                        "INTO"
                            | "OUTFILE"
                            | "DUMPFILE"
                            | "ANALYZE"
                            | "PROCEDURE"
                            | "LOCK"
                            | "UNLOCK"
                            | "SQL_CALC_FOUND_ROWS"
                    ) =>
            {
                return Err(REJECTED.into());
            }
            _ => {}
        }
    }
    // Doris-only administrative read views are deliberately exact token sequences.
    let significant: Vec<_> = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    let significant = significant
        .strip_suffix(&[&Token::SemiColon])
        .unwrap_or(&significant);
    if kind == DbKind::Doris
        && significant.len() == 2
        && significant
            .iter()
            .all(|t| matches!(t, Token::Word(w) if w.quote_style.is_none()))
    {
        let text = significant
            .iter()
            .map(|t| t.to_string().to_ascii_uppercase())
            .collect::<Vec<_>>();
        if text[0] == "SHOW" && matches!(text[1].as_str(), "BACKENDS" | "FRONTENDS") {
            return Ok(false);
        }
    }
    let statements = Parser::new(&dialect)
        .with_recursion_limit(48)
        .with_tokens(tokens)
        .parse_statements()
        .map_err(|_| REJECTED)?;
    if statements.len() != 1 {
        return Err(REJECTED.into());
    }
    if statements[0].visit(&mut ReadOnlyVisitor).is_break() {
        return Err(REJECTED.into());
    }
    Ok(matches!(statements[0], ast::Statement::Query(_)))
}
struct ReadOnlyVisitor;
impl Visitor for ReadOnlyVisitor {
    type Break = ();
    fn pre_visit_statement(&mut self, statement: &ast::Statement) -> ControlFlow<()> {
        use ast::Statement::*;
        let accepted = match statement {
            Query(_)
            | ShowDatabases { .. }
            | ShowSchemas { .. }
            | ShowTables { .. }
            | ShowColumns { .. }
            | ShowStatus { .. }
            | ShowVariables { .. }
            | ExplainTable { .. } => true,
            ShowCreate { obj_type, .. } => matches!(
                obj_type,
                ast::ShowCreateObject::Table | ast::ShowCreateObject::View
            ),
            Explain {
                analyze,
                statement,
                options,
                ..
            } => !analyze && options.is_none() && matches!(statement.as_ref(), Query(_)),
            _ => false,
        };
        if accepted {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    }
    fn pre_visit_query(&mut self, query: &ast::Query) -> ControlFlow<()> {
        if query.locks.is_empty()
            && query.for_clause.is_none()
            && query.settings.is_none()
            && query.format_clause.is_none()
            && query.pipe_operators.is_empty()
            && read_only_body(&query.body)
        {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    }
    fn pre_visit_relation(&mut self, relation: &ast::ObjectName) -> ControlFlow<()> {
        if relation.0.len() > 1
            && relation.0[0]
                .to_string()
                .trim_matches(['`', '"'])
                .eq_ignore_ascii_case("mysql")
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
    fn pre_visit_table_factor(&mut self, factor: &ast::TableFactor) -> ControlFlow<()> {
        match factor {
            ast::TableFactor::Table {
                args: None,
                with_hints,
                ..
            } if with_hints.is_empty() => ControlFlow::Continue(()),
            ast::TableFactor::Derived { .. } | ast::TableFactor::NestedJoin { .. } => {
                ControlFlow::Continue(())
            }
            _ => ControlFlow::Break(()),
        }
    }
    fn pre_visit_expr(&mut self, expr: &ast::Expr) -> ControlFlow<()> {
        if let ast::Expr::Function(function) = expr {
            // Reject qualified/UDF calls; a deny list alone cannot account for installed UDFs.
            let name = function.name.to_string().to_ascii_uppercase();
            if function.name.0.len() != 1
                || !matches!(
                    name.as_str(),
                    "COUNT"
                        | "SUM"
                        | "AVG"
                        | "MIN"
                        | "MAX"
                        | "ABS"
                        | "CEIL"
                        | "CEILING"
                        | "FLOOR"
                        | "ROUND"
                        | "TRUNCATE"
                        | "MOD"
                        | "POW"
                        | "POWER"
                        | "SQRT"
                        | "COALESCE"
                        | "IFNULL"
                        | "NULLIF"
                        | "IF"
                        | "GREATEST"
                        | "LEAST"
                        | "CONCAT"
                        | "CONCAT_WS"
                        | "LENGTH"
                        | "CHAR_LENGTH"
                        | "CHARACTER_LENGTH"
                        | "LOWER"
                        | "UPPER"
                        | "TRIM"
                        | "LTRIM"
                        | "RTRIM"
                        | "SUBSTRING"
                        | "SUBSTR"
                        | "LEFT"
                        | "RIGHT"
                        | "REPLACE"
                        | "REVERSE"
                        | "INSTR"
                        | "LOCATE"
                        | "DATE"
                        | "YEAR"
                        | "MONTH"
                        | "DAY"
                        | "DAYOFMONTH"
                        | "DAYOFWEEK"
                        | "HOUR"
                        | "MINUTE"
                        | "SECOND"
                        | "DATE_FORMAT"
                        | "STR_TO_DATE"
                        | "DATEDIFF"
                        | "TIMESTAMPDIFF"
                        | "DATE_ADD"
                        | "DATE_SUB"
                        | "NOW"
                        | "CURRENT_TIMESTAMP"
                        | "CURRENT_DATE"
                        | "CURDATE"
                        | "UNIX_TIMESTAMP"
                        | "FROM_UNIXTIME"
                        | "JSON_EXTRACT"
                        | "JSON_UNQUOTE"
                        | "JSON_LENGTH"
                        | "JSON_VALID"
                        | "GROUP_CONCAT"
                        | "ROW_NUMBER"
                        | "RANK"
                        | "DENSE_RANK"
                        | "LAG"
                        | "LEAD"
                        | "FIRST_VALUE"
                        | "LAST_VALUE"
                        | "VERSION"
                        | "DATABASE"
                        | "SCHEMA"
                )
            {
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    }
}
fn read_only_body(body: &ast::SetExpr) -> bool {
    match body {
        ast::SetExpr::Select(select) => select.into.is_none(),
        ast::SetExpr::Query(_) => true,
        ast::SetExpr::SetOperation { left, right, .. } => {
            read_only_body(left) && read_only_body(right)
        }
        _ => false,
    }
}

fn connect_options(config: &DbConfig) -> Result<MySqlConnectOptions, String> {
    if config.host.is_empty()
        || config.host.len() > 253
        || config.host.contains(['/', '@', '\\'])
        || config
            .host
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        || config.port == 0
        || config.username.is_empty()
        || config.username.len() > 128
        || config.password.len() > 16384
        || config.database.len() > 256
        || config.name.len() > 80
    {
        return Err("连接参数无效".into());
    }
    let ssl = match config.tls {
        DbTls::VerifyIdentity => MySqlSslMode::VerifyIdentity,
        DbTls::Required => MySqlSslMode::Required,
        DbTls::Disabled => MySqlSslMode::Disabled,
    };
    let mut options = MySqlConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .password(&config.password)
        .ssl_mode(ssl)
        .charset("utf8mb4")
        .statement_cache_capacity(0)
        .disable_statement_logging();
    if !config.database.is_empty() {
        options = options.database(&config.database);
    }
    Ok(options)
}
fn connection_error(error: sqlx::Error) -> String {
    match error {
        sqlx::Error::Database(error) => {
            let code = error
                .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
                .map(|e| e.number());
            match code {
                Some(1045 | 1698) => {
                    "数据库认证失败：用户名、密码或认证方式被拒绝，请核对凭据。".into()
                }
                Some(1044 | 1142 | 1227) => {
                    "数据库权限不足：当前账号无目标数据库或所需只读权限。".into()
                }
                Some(1049) => "目标数据库不存在，请核对默认数据库名称。".into(),
                Some(1130 | 1129) => {
                    "来源主机被数据库拒绝，请核对账号允许的来源主机及服务端访问控制。".into()
                }
                Some(1040 | 1203) => "数据库连接数已达上限，请释放连接或联系管理员。".into(),
                Some(3159) => "数据库要求安全传输，请核对TLS策略。".into(),
                Some(code) => {
                    format!("数据库拒绝连接（MySQL错误码{code}），请按错误码核对服务端日志。")
                }
                None => "数据库拒绝连接，请核对服务端访问控制和日志。".into(),
            }
        }
        sqlx::Error::Tls(error) => {
            // This exact message is generated by the locked driver, never reflected.
            if error.to_string() == "server does not support TLS" {
                "服务端未启用TLS，与当前TLS策略不匹配。请开启服务端TLS，或在可信网络中显式调整TLS策略。".into()
            } else {
                "TLS握手或证书验证失败，请核对证书信任链、主机名和服务端TLS配置。".into()
            }
        }
        sqlx::Error::Io(error) => {
            use std::io::ErrorKind;
            match error.kind() {
                ErrorKind::ConnectionRefused => {
                    "数据库拒绝连接，请核对主机、端口及服务监听状态。".into()
                }
                ErrorKind::TimedOut => {
                    "数据库网络连接超时，请核对网络路由、防火墙和服务状态。".into()
                }
                ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::BrokenPipe
                | ErrorKind::UnexpectedEof => {
                    "数据库连接被关闭，请核对服务端状态、代理和协议端口。".into()
                }
                ErrorKind::PermissionDenied => "本机网络访问被拒绝，请核对网络访问策略。".into(),
                _ if error
                    .to_string()
                    .starts_with("failed to lookup address information:") =>
                {
                    "数据库域名解析失败，请核对主机名和DNS配置。".into()
                }
                _ => "数据库网络连接失败，请核对地址、路由和防火墙。".into(),
            }
        }
        sqlx::Error::Protocol(_) => {
            "数据库协议握手失败，请核对是否为MySQL/Doris协议端口及认证插件兼容性。".into()
        }
        sqlx::Error::Configuration(_) | sqlx::Error::InvalidArgument(_) => {
            "数据库连接配置无效，请核对地址、数据库和TLS设置。".into()
        }
        _ => "数据库连接失败，请核对服务状态和驱动兼容性。".into(),
    }
}
async fn open(config: &DbConfig) -> Result<MySqlConnection, String> {
    let options = connect_options(config)?;
    tokio::time::timeout(
        Duration::from_secs(12),
        MySqlConnection::connect_with(&options),
    )
    .await
    .map_err(|_| "数据库连接超时")?
    .map_err(connection_error)
}
fn decode_row(row: &MySqlRow) -> Result<Vec<Option<String>>, String> {
    if row.len() > 256 {
        return Err("结果列超过 256 列上限".into());
    }
    let mut cells = Vec::with_capacity(row.len());
    let mut bytes = 0usize;
    for n in 0..row.len() {
        let raw = row.try_get_raw(n).map_err(|_| QUERY_ERROR)?;
        if raw.is_null() {
            cells.push(None);
            continue;
        }
        // raw_sql uses the text protocol, including exact decimal/integer strings.
        let raw: &[u8] = row.try_get_unchecked(n).map_err(|_| QUERY_ERROR)?;
        if raw.len() > 256 * 1024 {
            return Err("单元格超过 256 KiB 上限，查询已停止".into());
        }
        let value = match String::from_utf8(raw.to_vec()) {
            Ok(text) => text,
            Err(error) => {
                let mut text = String::from("0x");
                for byte in error.as_bytes() {
                    use std::fmt::Write;
                    let _ = write!(text, "{byte:02x}");
                }
                text
            }
        };
        bytes += value.len();
        if bytes > MAX_BYTES {
            return Err("单行结果超过内存上限".into());
        }
        cells.push(Some(value));
    }
    Ok(cells)
}
async fn read_result(
    config: &DbConfig,
    sql: &str,
    max_rows: usize,
    state: &QueryState,
) -> Result<(), String> {
    let mut connection = open(config).await?;
    if config.kind == DbKind::Mysql {
        connection
            .execute("START TRANSACTION READ ONLY")
            .await
            .map_err(|_| "MySQL 只读事务不可用，已拒绝执行")?;
    }
    {
        let mut stream = connection.fetch(sql);
        while let Some(row) = stream.try_next().await.map_err(|_| QUERY_ERROR)? {
            let cells = decode_row(&row)?;
            let mut record = state.record.lock().map_err(|_| QUERY_ERROR)?;
            if record.status != "running" {
                break;
            }
            if record.columns.is_empty() {
                record.columns = row
                    .columns()
                    .iter()
                    .map(|c| DbColumn {
                        name: c.name().chars().take(256).collect(),
                        data_type: c.type_info().name().into(),
                    })
                    .collect();
                record.bytes = record
                    .columns
                    .iter()
                    .map(|c| c.name.len() + c.data_type.len())
                    .sum();
            }
            let cell_bytes: usize = 24
                + cells
                    .iter()
                    .map(|c| c.as_ref().map_or(32, |v| v.len() + 32))
                    .sum::<usize>();
            if record.rows.len() >= max_rows.min(MAX_ROWS)
                || record.bytes.saturating_add(cell_bytes) > MAX_BYTES
            {
                record.truncated = true;
                break;
            }
            record.bytes += cell_bytes;
            record.rows.push(cells);
        }
    }
    let needs_columns = state
        .record
        .lock()
        .map_err(|_| QUERY_ERROR)?
        .columns
        .is_empty();
    if needs_columns {
        let description = connection
            .describe(sql)
            .await
            .map_err(|_| "空结果的字段描述不可用；服务端须支持预处理描述")?;
        let mut record = state.record.lock().map_err(|_| QUERY_ERROR)?;
        record.columns = description
            .columns()
            .iter()
            .map(|column| DbColumn {
                name: column.name().chars().take(256).collect(),
                data_type: column.type_info().name().into(),
            })
            .collect();
    }
    // Close the dedicated transport instead of draining a potentially huge result set.
    let _ = connection.close_hard().await;
    Ok(())
}
pub async fn test_connection(mut config: DbConfig) -> Result<(), String> {
    config.name.clear();
    let state = QueryState::new(String::new());
    tokio::time::timeout(
        Duration::from_secs(15),
        read_result(&config, "SELECT 1", 1, &state),
    )
    .await
    .map_err(|_| "数据库测试连接超时")??;
    Ok(())
}
#[tauri::command]
pub async fn db_test_connection(config: DbConfig) -> Result<(), String> {
    test_connection(config).await
}
impl DbService {
    fn config(&self, id: &str) -> Result<Arc<DbConfig>, String> {
        self.inner
            .lock()
            .map_err(|_| "连接状态不可用")?
            .sessions
            .get(id)
            .cloned()
            .ok_or_else(|| "连接已断开".into())
    }
    fn query_state(&self, id: &str) -> Result<Arc<QueryState>, String> {
        self.inner
            .lock()
            .map_err(|_| "查询状态不可用")?
            .queries
            .get(id)
            .cloned()
            .ok_or_else(|| "结果不存在或已释放".into())
    }
    pub(crate) async fn connect(&self, config: DbConfig) -> Result<DbSession, String> {
        if self
            .inner
            .lock()
            .map_err(|_| "连接状态不可用")?
            .sessions
            .len()
            >= 4
        {
            return Err("最多保留四个连接，请先断开旧连接".into());
        }
        let state = QueryState::new(String::new());
        tokio::time::timeout(
            Duration::from_secs(15),
            read_result(&config, "SELECT VERSION() AS version", 1, &state),
        )
        .await
        .map_err(|_| "数据库连接超时")??;
        let version = state
            .record
            .lock()
            .map_err(|_| QUERY_ERROR)?
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.clone())
            .unwrap_or_else(|| "unknown".into())
            .chars()
            .take(128)
            .collect();
        let id = Uuid::new_v4().to_string();
        let mut inner = self.inner.lock().map_err(|_| "连接状态不可用")?;
        if inner.sessions.len() >= 4 {
            return Err("最多保留四个连接，请先断开旧连接".into());
        }
        let kind = config.kind;
        let mut notes = vec![if kind == DbKind::Mysql {
            "MySQL：每次查询使用专用只读事务；取消直接关闭该查询连接".into()
        } else {
            "Doris：独立只读语法守卫；不发送 MySQL 事务/管理命令；不支持的扩展语法默认拒绝".into()
        }];
        notes.push(
            match config.tls {
                DbTls::VerifyIdentity => "TLS：验证系统 CA 证书和主机名",
                DbTls::Required => "TLS：强制加密但不验证证书身份",
                DbTls::Disabled => "TLS 已关闭，数据通过明文 TCP 传输",
            }
            .into(),
        );
        inner.sessions.insert(id.clone(), Arc::new(config));
        Ok(DbSession {
            id,
            kind,
            version,
            read_only_transaction: kind == DbKind::Mysql,
            notes,
        })
    }
    pub(crate) fn disconnect(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "连接状态不可用")?;
        inner.sessions.remove(id);
        if let Some((_, cancel)) = inner.metadata.remove(id) {
            let _ = cancel.send(true);
        }
        inner.queries.retain(|_, state| {
            if state.session_id == id {
                state.cancel();
                false
            } else {
                true
            }
        });
        Ok(())
    }
    fn start_query(
        &self,
        session_id: String,
        sql: String,
        max_rows: usize,
        timeout_secs: u64,
        explain: bool,
    ) -> Result<String, String> {
        if max_rows == 0 || max_rows > MAX_ROWS || timeout_secs == 0 || timeout_secs > 120 {
            return Err("行数范围为 1–10000，超时范围为 1–120 秒".into());
        }
        let config = self.config(&session_id)?;
        let is_select = validate_read_only(&sql, config.kind)?;
        if explain && !is_select {
            return Err("执行计划只接受 SELECT 或只读 CTE".into());
        }
        let sql = if explain {
            format!("EXPLAIN {sql}")
        } else {
            sql
        };
        let id = Uuid::new_v4().to_string();
        let state = Arc::new(QueryState::new(session_id.clone()));
        let mut inner = self.inner.lock().map_err(|_| "查询状态不可用")?;
        if !inner.sessions.contains_key(&session_id) {
            return Err("连接已断开".into());
        }
        if inner.queries.len() >= MAX_QUERIES {
            return Err("最多缓存四个查询，请先释放旧结果".into());
        }
        if inner.queries.values().any(|q| {
            q.session_id == session_id
                && q.record
                    .lock()
                    .map(|r| r.status == "running")
                    .unwrap_or(true)
        }) {
            return Err("该连接已有正在运行的查询".into());
        }
        inner.queries.insert(id.clone(), state.clone());
        let running = state.clone();
        // Synchronous Tauri commands run on the desktop event thread, outside Tokio.
        let handle = tauri::async_runtime::spawn(async move {
            let result = tokio::time::timeout(
                Duration::from_secs(timeout_secs),
                read_result(&config, &sql, max_rows, &running),
            )
            .await
            .unwrap_or_else(|_| Err("查询超时，专用数据库连接已关闭".into()));
            running.finish(result);
        });
        *state.abort.lock().map_err(|_| "取消状态不可用")? = Some(handle.inner().abort_handle());
        Ok(id)
    }
    async fn metadata(&self, session_id: &str, sql: String) -> Result<DbPage, String> {
        let id = Uuid::new_v4().to_string();
        let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let config = {
            let mut inner = self.inner.lock().map_err(|_| "连接状态不可用")?;
            let config = inner
                .sessions
                .get(session_id)
                .cloned()
                .ok_or("连接已断开")?;
            if inner.metadata.contains_key(session_id) {
                return Err("目录查询正在运行，请稍后重试".into());
            }
            inner
                .metadata
                .insert(session_id.into(), (id.clone(), cancel));
            config
        };
        let _lease = MetadataLease {
            service: self,
            session_id: session_id.into(),
            id,
        };
        let state = QueryState::new(session_id.into());
        let result = tokio::select! {
            biased;
            _=cancelled.changed()=>Err("目录查询已取消".into()),
            result=tokio::time::timeout(Duration::from_secs(20),read_result(&config,&sql,2000,&state))=>result.map_err(|_|"目录查询超时")?,
        };
        state.finish(result.clone());
        result?;
        // Metadata is independently bounded and intentionally returned as one table.
        let record = state.record.lock().map_err(|_| QUERY_ERROR)?;
        Ok(DbPage {
            query_id: String::new(),
            columns: record.columns.clone(),
            rows: record.rows.clone(),
            total: record.rows.len(),
            offset: 0,
            limit: record.rows.len().max(1),
            truncated: record.truncated,
            elapsed_ms: record.elapsed_ms,
            status: record.status.into(),
            error: record.error.clone(),
        })
    }
    fn export_csv(&self, id: &str) -> Result<String, String> {
        let state = self.query_state(id)?;
        let record = state.record.lock().map_err(|_| QUERY_ERROR)?;
        if record.status == "running" {
            return Err("请等待查询结束后再导出".into());
        }
        let mut csv = record
            .columns
            .iter()
            .map(|c| csv_cell(Some(&c.name)))
            .collect::<Vec<_>>()
            .join(",");
        csv.push_str("\r\n");
        for row in &record.rows {
            csv.push_str(
                &row.iter()
                    .map(|c| csv_cell(c.as_deref()))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            csv.push_str("\r\n");
        }
        Ok(csv)
    }
}

#[tauri::command]
pub async fn db_connect(
    service: tauri::State<'_, DbService>,
    config: DbConfig,
) -> Result<DbSession, String> {
    service.connect(config).await
}
#[tauri::command]
pub fn db_disconnect(
    service: tauri::State<'_, DbService>,
    session_id: String,
) -> Result<(), String> {
    service.disconnect(&session_id)
}
#[tauri::command]
pub async fn db_databases(
    service: tauri::State<'_, DbService>,
    session_id: String,
) -> Result<DbCatalog, String> {
    let page = service
        .metadata(&session_id, "SHOW DATABASES".into())
        .await?;
    Ok(DbCatalog {
        names: page
            .rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .filter(|n| !n.eq_ignore_ascii_case("mysql"))
            .collect(),
        truncated: page.truncated,
    })
}
#[tauri::command]
pub async fn db_tables(
    service: tauri::State<'_, DbService>,
    session_id: String,
    database: String,
) -> Result<DbCatalog, String> {
    if database.eq_ignore_ascii_case("mysql") {
        return Err(REJECTED.into());
    }
    let page = service
        .metadata(
            &session_id,
            format!("SHOW TABLES FROM {}", identifier(&database)?),
        )
        .await?;
    Ok(DbCatalog {
        names: page
            .rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect(),
        truncated: page.truncated,
    })
}
#[tauri::command]
pub async fn db_describe(
    service: tauri::State<'_, DbService>,
    session_id: String,
    database: String,
    table: String,
) -> Result<DbInspection, String> {
    if database.eq_ignore_ascii_case("mysql") {
        return Err(REJECTED.into());
    }
    let qualified = format!("{}.{}", identifier(&database)?, identifier(&table)?);
    let columns = service
        .metadata(&session_id, format!("SHOW FULL COLUMNS FROM {qualified}"))
        .await?;
    let indexes = service
        .metadata(&session_id, format!("SHOW INDEX FROM {qualified}"))
        .await?;
    Ok(DbInspection { columns, indexes })
}
#[tauri::command]
pub fn db_query(
    service: tauri::State<'_, DbService>,
    session_id: String,
    sql: String,
    max_rows: usize,
    timeout_secs: u64,
    explain: bool,
) -> Result<String, String> {
    service.start_query(session_id, sql, max_rows, timeout_secs, explain)
}
#[tauri::command]
pub fn db_query_page(
    service: tauri::State<'_, DbService>,
    query_id: String,
    offset: usize,
    limit: usize,
) -> Result<DbPage, String> {
    service
        .query_state(&query_id)?
        .page(query_id, offset, limit)
}
#[tauri::command]
pub fn db_cancel(service: tauri::State<'_, DbService>, query_id: String) -> Result<(), String> {
    service.query_state(&query_id)?.cancel();
    Ok(())
}
#[tauri::command]
pub fn db_export_csv(
    service: tauri::State<'_, DbService>,
    query_id: String,
) -> Result<String, String> {
    service.export_csv(&query_id)
}
#[tauri::command]
pub fn db_forget_query(
    service: tauri::State<'_, DbService>,
    query_id: String,
) -> Result<(), String> {
    if let Some(state) = service
        .inner
        .lock()
        .map_err(|_| "查询状态不可用")?
        .queries
        .remove(&query_id)
    {
        state.cancel();
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guard_rejects_writes_side_effects_and_parser_escapes() {
        for sql in [
            "DELETE FROM t",
            "SELECT 1; SELECT 2",
            "WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x",
            "WITH x AS (SELECT 1) UPDATE t SET x=1",
            "EXPLAIN DELETE FROM t",
            "EXPLAIN ANALYZE SELECT 1",
            "SELECT * FROM t INTO OUTFILE '/var/a'",
            "SELECT * INTO newtable FROM t",
            "SELECT 1 INTO @x",
            "SELECT * FROM t FOR UPDATE",
            "SELECT * FROM t LOCK IN SHARE MODE",
            "SELECT SLEEP(1)",
            "SELECT BENCHMARK(1000, 1)",
            "SELECT GET_LOCK('x',1)",
            "SELECT LOAD_FILE('/a')",
            "SELECT RELEASE_LOCK('x')",
            "SELECT evil_udf(1)",
            "SELECT sys.evil(1)",
            "SELECT 1 /*! UNION SELECT 2 */",
            "SELECT /*+ SET_VAR(x=1) */ 1",
            "SELECT @x:=1",
            "SELECT * FROM mysql.user",
            "SELECT * FROM FILES('s3.uri'='s3://x')",
        ] {
            assert!(
                validate_read_only(sql, DbKind::Mysql).is_err(),
                "accepted unsafe SQL: {sql}"
            );
            assert!(
                validate_read_only(sql, DbKind::Doris).is_err(),
                "Doris accepted unsafe SQL: {sql}"
            );
        }
    }
    #[test]
    fn guard_accepts_nested_read_queries_without_confusing_string_literals() {
        for sql in [
            "SELECT 1",
            "SELECT 'DELETE; SLEEP(3)' AS text",
            "SELECT count(*), max(id) FROM demo.t",
            "WITH x AS (SELECT 1 AS a) SELECT a FROM x",
            "SELECT * FROM (SELECT 1) AS t UNION ALL SELECT 2",
            "SHOW DATABASES",
            "SHOW TABLES FROM demo",
            "DESCRIBE demo.t",
            "EXPLAIN SELECT * FROM t",
        ] {
            assert!(
                validate_read_only(sql, DbKind::Mysql).is_ok(),
                "rejected safe SQL: {sql}"
            );
        }
        assert!(validate_read_only("SHOW BACKENDS", DbKind::Doris).is_ok());
        assert!(validate_read_only("SHOW BACKENDS", DbKind::Mysql).is_err());
        assert!(validate_read_only("SHOW BACKENDS; DROP TABLE t", DbKind::Doris).is_err());
    }
    #[test]
    fn pages_enforce_bounds_and_do_not_slice_past_results() {
        assert_eq!(page_bounds(21, 10, 23).unwrap(), (20, 23));
        assert_eq!(page_bounds(999, 1000, 23).unwrap(), (0, 23));
        assert!(page_bounds(0, 0, 0).is_err());
    }
    #[test]
    fn export_escapes_delimiters_and_neutralizes_formulas() {
        assert_eq!(csv_cell(Some("=1+1")), "\"'=1+1\"");
        assert_eq!(csv_cell(Some("a,\"b\n")), "\"a,\"\"b\n\"");
        assert_eq!(csv_cell(None), "\"\"");
    }
    // The fixture speaks the actual MySQL text protocol and only binds loopback.
    async fn wire_packet(
        stream: &mut tokio::net::TcpStream,
        sequence: u8,
        body: &[u8],
    ) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        let n = body.len();
        stream
            .write_all(&[n as u8, (n >> 8) as u8, (n >> 16) as u8, sequence])
            .await?;
        stream.write_all(body).await
    }
    async fn read_packet(stream: &mut tokio::net::TcpStream) -> std::io::Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).await?;
        let n = header[0] as usize | (header[1] as usize) << 8 | (header[2] as usize) << 16;
        if n > 65536 {
            return Err(std::io::Error::other("fixture packet limit"));
        }
        let mut body = vec![0; n];
        stream.read_exact(&mut body).await?;
        Ok(body)
    }
    fn length_string(value: &str, target: &mut Vec<u8>) {
        let len = value.len();
        if len < 251 {
            target.push(len as u8);
        } else if len <= 65535 {
            target.push(252);
            target.extend((len as u16).to_le_bytes());
        } else {
            target.push(253);
            target.extend(&(len as u32).to_le_bytes()[..3]);
        }
        target.extend(value.as_bytes());
    }
    async fn wire_result(stream: &mut tokio::net::TcpStream, rows: &[&str]) -> std::io::Result<()> {
        wire_packet(stream, 1, &[1]).await?;
        let mut column = vec![];
        for text in ["def", "demo", "items", "items", "value", "value"] {
            length_string(text, &mut column);
        }
        column.extend([12, 33, 0, 255, 0, 0, 0, 253, 0, 0, 0, 0, 0]);
        wire_packet(stream, 2, &column).await?;
        let eof = [254, 0, 0, 2, 0];
        wire_packet(stream, 3, &eof).await?;
        let mut sequence = 4;
        for value in rows {
            let mut row = vec![];
            length_string(value, &mut row);
            wire_packet(stream, sequence, &row).await?;
            sequence += 1;
        }
        wire_packet(stream, sequence, &eof).await
    }
    async fn fixture_connection(
        mut stream: tokio::net::TcpStream,
        trace: Arc<Mutex<Vec<String>>>,
        closed: Arc<tokio::sync::Notify>,
    ) -> std::io::Result<()> {
        let capabilities: u32 = 1 | 4 | 8 | 512 | 8192 | 32768 | (1 << 19);
        let mut hello = vec![10];
        hello.extend(b"8.0.36-fixture\0");
        hello.extend(1u32.to_le_bytes());
        hello.extend(b"12345678\0");
        hello.extend((capabilities as u16).to_le_bytes());
        hello.push(45);
        hello.extend(2u16.to_le_bytes());
        hello.extend(((capabilities >> 16) as u16).to_le_bytes());
        hello.push(21);
        hello.extend([0; 10]);
        hello.extend(b"abcdefghijkl\0mysql_native_password\0");
        wire_packet(&mut stream, 0, &hello).await?;
        let _auth = read_packet(&mut stream).await?;
        let ok = [0, 0, 0, 2, 0, 0, 0];
        wire_packet(&mut stream, 2, &ok).await?;
        loop {
            let packet = match read_packet(&mut stream).await {
                Ok(p) => p,
                Err(_) => return Ok(()),
            };
            if packet.first() == Some(&1) {
                return Ok(());
            }
            if packet.first() == Some(&22) {
                let prepared = [0, 42, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
                wire_packet(&mut stream, 1, &prepared).await?;
                let mut column = vec![];
                for text in ["def", "demo", "empty", "empty", "value", "value"] {
                    length_string(text, &mut column);
                }
                column.extend([12, 33, 0, 255, 0, 0, 0, 253, 0, 0, 0, 0, 0]);
                wire_packet(&mut stream, 2, &column).await?;
                wire_packet(&mut stream, 3, &[254, 0, 0, 2, 0]).await?;
                continue;
            }
            if packet.first() == Some(&25) {
                continue;
            }
            if packet.first() != Some(&3) {
                return Err(std::io::Error::other("fixture expects COM_QUERY"));
            }
            let sql = String::from_utf8_lossy(&packet[1..]).into_owned();
            trace.lock().unwrap().push(sql.clone());
            if sql.contains("999") {
                let _ = read_packet(&mut stream).await;
                closed.notify_one();
                return Ok(());
            } else if sql.starts_with("SET ") || sql == "START TRANSACTION READ ONLY" {
                wire_packet(&mut stream, 1, &ok).await?;
            } else if sql.contains("VERSION()") {
                wire_result(&mut stream, &["8.0.36-fixture"]).await?;
            } else if sql == "SHOW DATABASES" {
                wire_result(&mut stream, &["demo"]).await?;
            } else if sql.starts_with("SHOW TABLES") {
                wire_result(&mut stream, &["items"]).await?;
            } else if sql.starts_with("SHOW FULL COLUMNS") {
                wire_result(&mut stream, &["id"]).await?;
            } else if sql.starts_with("SHOW INDEX") {
                wire_result(&mut stream, &["PRIMARY"]).await?;
            } else if sql.contains("oversize") {
                let value = "x".repeat(300 * 1024);
                wire_result(&mut stream, &[&value]).await?;
            } else if sql.contains("FROM empty") {
                wire_result(&mut stream, &[]).await?;
            } else {
                wire_result(&mut stream, &["=formula", "two", "three", "four"]).await?;
            }
        }
    }
    async fn wire_fixture() -> (
        DbConfig,
        Arc<Mutex<Vec<String>>>,
        Arc<tokio::sync::Notify>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let trace = Arc::new(Mutex::new(vec![]));
        let closed = Arc::new(tokio::sync::Notify::new());
        let port = listener.local_addr().unwrap().port();
        let log = trace.clone();
        let notifier = closed.clone();
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let log = log.clone();
                let notify = notifier.clone();
                tokio::spawn(async move {
                    let _ = fixture_connection(stream, log, notify).await;
                });
            }
        });
        (
            DbConfig {
                name: "fixture".into(),
                host: "127.0.0.1".into(),
                port,
                username: "readonly".into(),
                password: "fixture-only".into(),
                database: "demo".into(),
                kind: DbKind::Mysql,
                tls: DbTls::Disabled,
            },
            trace,
            closed,
            task,
        )
    }
    async fn wait_finished(service: &DbService, id: &str) -> DbPage {
        for _ in 0..100 {
            let page = service
                .query_state(id)
                .unwrap()
                .page(id.into(), 0, 50)
                .unwrap();
            if page.status != "running" {
                return page;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("fixture query did not finish");
    }
    #[tokio::test]
    async fn desktop_query_starts_from_a_thread_without_a_tokio_context() {
        let (config, _, _, listener) = wire_fixture().await;
        let service = Arc::new(DbService::default());
        let session = service.connect(config).await.unwrap();
        let caller = service.clone();
        let session_id = session.id.clone();
        let result = std::thread::spawn(move || {
            assert!(tokio::runtime::Handle::try_current().is_err());
            caller.start_query(session_id, "SELECT 1".into(), 100, 5, false)
        })
        .join();
        let id = result
            .expect("synchronous desktop IPC must not panic outside Tokio")
            .unwrap();
        let page = wait_finished(&service, &id).await;
        assert_eq!(page.status, "completed");
        assert_eq!(page.total, 4);
        service.disconnect(&session.id).unwrap();
        listener.abort();
    }

    #[tokio::test]
    async fn protocol_mysql_reads_metadata_bounds_results_and_exports_exact_text() {
        let (config, trace, _, listener) = wire_fixture().await;
        let service = DbService::default();
        let session = service.connect(config).await.unwrap();
        assert!(session.read_only_transaction);
        let names = service
            .metadata(&session.id, "SHOW DATABASES".into())
            .await
            .unwrap();
        assert_eq!(names.rows, vec![vec![Some("demo".into())]]);
        for sql in [
            "SHOW TABLES FROM `demo`",
            "SHOW FULL COLUMNS FROM `demo`.`items`",
            "SHOW INDEX FROM `demo`.`items`",
        ] {
            assert_eq!(
                service
                    .metadata(&session.id, sql.into())
                    .await
                    .unwrap()
                    .rows
                    .len(),
                1
            );
        }
        let id = service
            .start_query(
                session.id.clone(),
                "SELECT * FROM items".into(),
                3,
                5,
                false,
            )
            .unwrap();
        let page = wait_finished(&service, &id).await;
        assert_eq!(page.status, "completed");
        assert!(page.truncated);
        assert_eq!(page.total, 3);
        assert_eq!(page.rows[0][0].as_deref(), Some("=formula"));
        let second = service
            .query_state(&id)
            .unwrap()
            .page(id.clone(), 2, 2)
            .unwrap();
        assert_eq!(second.offset, 2);
        assert_eq!(second.rows, vec![vec![Some("three".into())]]);
        assert_eq!(
            service.export_csv(&id).unwrap(),
            "\"value\"\r\n\"'=formula\"\r\n\"two\"\r\n\"three\"\r\n"
        );
        assert!(
            trace
                .lock()
                .unwrap()
                .iter()
                .any(|s| s == "START TRANSACTION READ ONLY")
        );
        assert!(
            service
                .start_query(session.id.clone(), "DELETE FROM items".into(), 2, 5, false)
                .is_err()
        );
        service.disconnect(&session.id).unwrap();
        assert!(service.query_state(&id).is_err());
        listener.abort();
    }
    #[tokio::test]
    async fn protocol_doris_never_sends_mysql_transaction_controls_and_cancel_closes_socket() {
        let (mut config, trace, closed, listener) = wire_fixture().await;
        config.kind = DbKind::Doris;
        let service = DbService::default();
        let session = service.connect(config).await.unwrap();
        assert!(!session.read_only_transaction);
        let id = service
            .start_query(
                session.id.clone(),
                "SELECT 999 AS stall".into(),
                2,
                5,
                false,
            )
            .unwrap();
        for _ in 0..100 {
            if trace.lock().unwrap().iter().any(|s| s.contains("999")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        service.query_state(&id).unwrap().cancel();
        tokio::time::timeout(Duration::from_secs(2), closed.notified())
            .await
            .expect("cancel must close dedicated socket");
        assert_eq!(
            service
                .query_state(&id)
                .unwrap()
                .page(id.clone(), 0, 50)
                .unwrap()
                .status,
            "cancelled"
        );
        assert!(
            !trace
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.contains("TRANSACTION") || s == "ROLLBACK")
        );
        service.disconnect(&session.id).unwrap();
        listener.abort();
    }
    #[tokio::test]
    async fn protocol_timeout_closes_socket_and_tls_does_not_silently_fall_back() {
        let (config, _, closed, listener) = wire_fixture().await;
        let service = DbService::default();
        let session = service.connect(config.clone()).await.unwrap();
        let id = service
            .start_query(
                session.id.clone(),
                "SELECT 999 AS stall".into(),
                2,
                1,
                false,
            )
            .unwrap();
        let page = wait_finished(&service, &id).await;
        assert_eq!(page.status, "failed");
        assert!(page.error.unwrap().contains("超时"));
        tokio::time::timeout(Duration::from_secs(2), closed.notified())
            .await
            .unwrap();
        let mut secure = config;
        secure.tls = DbTls::VerifyIdentity;
        let error = service
            .connect(secure)
            .await
            .err()
            .expect("fixture has no TLS; default must refuse");
        assert!(!error.contains("fixture-only"));
        assert!(!error.contains("readonly"));
        service.disconnect(&session.id).unwrap();
        listener.abort();
    }
    #[tokio::test]
    async fn metadata_is_bounded_per_session_and_disconnect_cancels_its_socket() {
        let (config, trace, closed, listener) = wire_fixture().await;
        let service = DbService::default();
        let session = service.connect(config).await.unwrap();
        let result = tokio::join!(
            service.metadata(&session.id, "SHOW TABLES FROM `999`".into()),
            async {
                for _ in 0..100 {
                    if trace.lock().unwrap().iter().any(|q| q.contains("999")) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                assert!(
                    service
                        .metadata(&session.id, "SHOW DATABASES".into())
                        .await
                        .is_err(),
                    "parallel metadata must be bounded"
                );
                service.disconnect(&session.id).unwrap();
            }
        );
        assert!(result.0.err().unwrap().contains("取消"));
        tokio::time::timeout(Duration::from_secs(2), closed.notified())
            .await
            .expect("disconnect closes metadata transport");
        listener.abort();
    }
    #[tokio::test]
    async fn zero_row_query_keeps_column_headers_and_can_export_them() {
        let (config, _, _, listener) = wire_fixture().await;
        let service = DbService::default();
        let session = service.connect(config).await.unwrap();
        let id = service
            .start_query(
                session.id.clone(),
                "SELECT value FROM empty".into(),
                10,
                5,
                false,
            )
            .unwrap();
        let page = wait_finished(&service, &id).await;
        assert_eq!(page.status, "completed");
        assert_eq!(page.total, 0);
        assert_eq!(
            page.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["value"]
        );
        assert_eq!(service.export_csv(&id).unwrap(), "\"value\"\r\n");
        service.disconnect(&session.id).unwrap();
        listener.abort();
    }

    #[tokio::test]
    async fn oversize_cell_is_rejected_without_entering_the_result_cache() {
        let (config, _, _, listener) = wire_fixture().await;
        let service = DbService::default();
        let session = service.connect(config).await.unwrap();
        let id = service
            .start_query(
                session.id.clone(),
                "SELECT oversize FROM items".into(),
                10,
                5,
                false,
            )
            .unwrap();
        let page = wait_finished(&service, &id).await;
        assert_eq!(page.status, "failed");
        assert_eq!(page.total, 0);
        assert!(page.error.unwrap().contains("256 KiB"));
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines().filter(|line| line.starts_with("VmHWM:")) {
                eprintln!("large_cell_fixture_process_{line}");
            }
        }
        service.disconnect(&session.id).unwrap();
        listener.abort();
    }
    #[tokio::test]
    async fn draft_database_probe_is_fixed_read_only_and_closes_its_dedicated_transport() {
        for kind in [DbKind::Mysql, DbKind::Doris] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let trace = Arc::new(Mutex::new(vec![]));
            let log = trace.clone();
            let (done, closed) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let _ = fixture_connection(stream, log, Arc::new(tokio::sync::Notify::new())).await;
                let _ = done.send(());
            });
            test_connection(DbConfig {
                name: String::new(),
                host: "127.0.0.1".into(),
                port,
                username: "readonly".into(),
                password: "fixture-only".into(),
                database: String::new(),
                kind,
                tls: DbTls::Disabled,
            })
            .await
            .unwrap();
            tokio::time::timeout(Duration::from_secs(2), closed)
                .await
                .expect("test must close its own connection")
                .unwrap();
            let log = trace.lock().unwrap();
            assert!(log.iter().any(|sql| sql == "SELECT 1"));
            assert!(!log.iter().any(|sql| sql.contains("VERSION()")));
            assert_eq!(
                log.iter().any(|sql| sql == "START TRANSACTION READ ONLY"),
                kind == DbKind::Mysql
            );
            server.await.unwrap();
        }
    }
    #[tokio::test]
    async fn connection_errors_distinguish_server_codes_without_reflecting_messages() {
        for (code, expected) in [
            (1045u16, "认证失败"),
            (1044, "权限"),
            (1049, "数据库不存在"),
            (1130, "来源主机"),
            (1040, "连接数"),
            (5999, "5999"),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut packet = vec![255];
                packet.extend(code.to_le_bytes());
                packet.extend(b"#28000fixture-sensitive-password-url-sentinel");
                wire_packet(&mut stream, 0, &packet).await.unwrap();
            });
            let error = open(&DbConfig {
                name: String::new(),
                host: "127.0.0.1".into(),
                port,
                username: "reader".into(),
                password: "fixture-sensitive-password-url-sentinel".into(),
                database: String::new(),
                kind: DbKind::Doris,
                tls: DbTls::Disabled,
            })
            .await
            .err()
            .unwrap();
            server.await.unwrap();
            assert!(!error.contains("fixture-sensitive"));
            assert!(
                error.contains(expected),
                "code {code} lost its safe category: {error}"
            );
        }
    }
    #[tokio::test]
    async fn connection_errors_identify_unsupported_tls_and_refused_socket() {
        let (mut config, _, _, server) = wire_fixture().await;
        config.tls = DbTls::VerifyIdentity;
        let error = open(&config).await.err().unwrap();
        assert!(
            error.contains("TLS") && error.contains("未启用TLS"),
            "unsupported TLS needs an actionable category: {error}"
        );
        server.abort();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        config.port = listener.local_addr().unwrap().port();
        drop(listener);
        config.tls = DbTls::Disabled;
        let error = open(&config).await.err().unwrap();
        assert!(
            error.contains("拒绝连接"),
            "refused listener must be distinguishable: {error}"
        );
    }

    #[test]
    fn typed_connection_errors_never_reflect_untrusted_text() {
        for (error, category) in [
            (
                sqlx::Error::Tls("fixture-sensitive-url-and-certificate".into()),
                "TLS",
            ),
            (
                sqlx::Error::Protocol("fixture-sensitive-url-and-certificate".into()),
                "协议",
            ),
            (
                sqlx::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "fixture-sensitive-url-and-certificate",
                )),
                "超时",
            ),
            (
                sqlx::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "fixture-sensitive-url-and-certificate",
                )),
                "关闭",
            ),
            (
                sqlx::Error::Io(std::io::Error::other(
                    "failed to lookup address information: fixture-sensitive-url-and-certificate",
                )),
                "域名解析",
            ),
        ] {
            let text = connection_error(error);
            assert!(text.contains(category));
            assert!(!text.contains("fixture-sensitive"));
        }
    }
}
