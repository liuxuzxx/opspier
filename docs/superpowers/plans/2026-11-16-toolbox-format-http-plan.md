# OpsPier 工具箱移植 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 smart-agent 的格式化工具和 API 请求调试工具按原有交互移植到 OpsPier 0.1.13。

**Architecture:** 新增独立工具工作区类型，前端保持 smart-agent 的工具栏、双栏编辑器和响应查看布局；格式化逻辑放在前端工具模块，HTTP 由新增 Tauri Rust 命令执行并返回安全截断后的响应快照。现有连接工作区和配置凭据生命周期不改变。

**Tech Stack:** React、Ant Design、Tauri 2、Rust reqwest、serde_json、serde_yaml、sqlparser。

**Spec:** `docs/superpowers/specs/2026-11-16-toolbox-format-http-design.md`

## Global Constraints

- 页面交互以 smart-agent 当前 FormatTool 和 ApiTester 为基线。
- HTTP 只允许 `http`/`https`，总超时 10 秒，响应正文上限 4 MiB，超出后截断并显示提示。
- 请求内容、Header、Token、Cookie 不持久化。
- 工具工作区与 SSH、数据库、Kubernetes、Nacos、MinIO 工作区互不共享状态。
- 不修改 smart-agent 源码。

## Review Focus

- 非法 JSON、XML、YAML、TOML、SQL 输入必须保留原输入并显示错误。
- Header 值中的冒号、重复 Header 和空值不能被错误拆分。
- HTTP 非 2xx 响应仍要展示状态、Header、Body；网络错误和超时要分类提示。
- 超大响应不能导致桌面端内存无限增长。
- 切换或关闭工具页后，迟到的 HTTP 响应不能写入新工作区。

---

### Task 1: 格式化核心与页面

**Files:**
- Create: `src/toolbox-format.ts`, `src/ToolboxFormatPanel.tsx`
- Test: `tests/toolbox-format.test.ts`
- Modify: `src/style.css`

**Interfaces:**
- `formatToolContent(type, input, mode, indent): string`
- `formatToolTypes`: JSON/XML/TOML/YAML/SQL/Markdown

- [ ] 写格式化核心的失败测试，覆盖六种类型、JSON 缩进、压缩、错误保留输入。
- [ ] 实现最小格式化函数和错误消息，不让失败结果覆盖输入。
- [ ] 按 smart-agent 样式实现顶部类型切换、格式化/压缩/复制/清空、左右编辑区。
- [ ] 运行 `npm test -- tests/toolbox-format.test.ts`，确认通过。

### Task 2: HTTP 请求命令与消息模型

**Files:**
- Create: `src/http-tool.ts`, `src-tauri/src/http_tool.rs`
- Test: `tests/http-tool.test.ts`, `src-tauri/src/http_tool_tests.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml`

**Interfaces:**
- Rust command `http_tool_request(request: HttpToolRequest) -> Result<HttpToolResponse, String>`
- 前端 `parseHeaderText(text): Record<string,string>`
- 前端 `formatHttpResponse(response): string`

- [ ] 先写 Header、响应分段、URL scheme、响应截断和非 2xx 保留的失败测试。
- [ ] 实现 Rust reqwest 请求：方法、URL、Header、可选 Body、10 秒总超时、4 MiB 响应上限。
- [ ] 仅允许 HTTP/HTTPS，使用默认 TLS 校验和默认重定向策略；不写入凭据。
- [ ] 注册 Tauri 命令并运行 Rust 单元测试。

### Task 3: HTTP 调试页面

**Files:**
- Create: `src/HttpToolPanel.tsx`
- Test: `tests/http-tool-ui.test.ts`
- Modify: `src/ConnectionManager.tsx`, `src/style.css`

**Interfaces:**
- 工作区 `kind: 'format' | 'http'`，状态仅存在当前 React 工作区。
- 页面提供 GET/POST/PUT/PATCH/DELETE、URL、Header、Body、发送、响应和复制。

- [ ] 写页面状态测试，覆盖发送锁、非法 JSON 不发请求、迟到响应丢弃。
- [ ] 实现 smart-agent 风格的 API 调试页面和错误/耗时提示。
- [ ] 将工具入口放入左侧“工具箱”，支持打开、切换、关闭多个工具页。
- [ ] 运行前端测试并验证现有连接工作区未被重挂载。

### Task 4: 版本、构建与桌面回归

**Files:**
- Modify: `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/tauri.conf.json`
- Test: `tests/toolbox_desktop.py`

- [ ] 将版本从 0.1.12 升到 0.1.13并运行版本一致性检查。
- [ ] 运行完整前端测试、Rust 测试、锁定离线构建。
- [ ] 在隔离桌面中验证格式化页面、HTTP fixture 请求、响应复制、窗口切换和清理。
- [ ] 保留当前用户运行实例，不自动替换默认入口。
