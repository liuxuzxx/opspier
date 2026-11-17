# OpsPier 0.1.9 Nacos 与 kubectl 实施计划

> **For agentic workers:** 使用 executing-plans 内联逐项执行；用户已批准方案与后台实施，无需重复确认。只由本实施者写产品，最终交根安排唯一独立审查。

**Goal:** 完成 Nacos 管理与 Kubernetes 完整 PTY 终端，同版交付 0.1.9 Linux 可运行包。
**Architecture:** 续接现有 Nacos 认证/连接/系统凭据代码，以具体目标和版本预检、单次写入、读回核验提供配置与服务编辑。独立 portable-pty 服务关联 Kubernetes sessionId/generation；只导出当前 Context 的自包含私有配置，终端与资源视图分别保留状态。
**Tech Stack:** 原 Rust/Tauri/React/AntD/xterm；新增锁定 portable-pty、fs2 及必要 Windows 平台 API；其他模块不重构。
**Spec:** ../specs/2026-11-13-nacos-kubectl-0.1.9-design.md（原文“草稿/待评审”为历史，签收已由交接确认）。

## 当前交付状态

2026-11-13：Tasks 1–4 的 Linux 功能及 Task 5 的 Linux 构建、独立审查和整应用回归已完成。前端 102 项、Rust 128 项、真实 Nacos 集成、真实 kubectl 协议和整应用 57 项 UI/2 项 IPC 检查通过。另修复命名空间误取消、终端误关闭、CRD 大页及输入失败后残缺命令问题。

下方清单保留初始实施拆分；本轮结果和验证边界以 [验证记录](../verification/2026-11-13-opspier-0.1.9.md) 为准。Windows 仅平台模块交叉编译通过，完整实机未验收；Git 提交、推送和发布未执行。

## Global Constraints

- 保留接手时所有未提交 Nacos 改动；不 reset/clean，不触碰其他项目。沿当前已授权工作目录实施，先保存源码快照。
- 当前 0.1.8 用户进程不停止、不操作；自动验证仅自有回环服务、私有 HOME/XDG/DBus/:196，禁 :0/:199、生产与用户默认集群。
- 终端支持完整 shell/RBAC 写命令，不新增只读白名单；图形 Kubernetes 仍只读。界面分别说明默认目标和用户可显式覆盖参数。
- 配置使用 App 私有目录、Unix 0700/0600；不修改宿主环境、默认 kubeconfig，不输出或诊断记录命令/配置/凭据。
- namespace 非空原子更新私有配置供下条命令；空表示 kubectl 默认 default，-A 由用户输入。Context 改变终止旧进程，新 Context 需重新打开终端。
- 输入单次64KiB，待写128KiB，输出512KiB背压，poll64KiB；最多8终端、每 Kubernetes 工作区1终端。路径/参数/环境分别传递。
- Nacos 2.x：配置内容保真、已有属性保留、CAS/快照防陈旧、所有写入读回核验，不自动重放不确定写入。不得擅增服务注册/注销。
- 版本仅一次0.1.8→0.1.9。Linux实测；Windows构建和实机证据分开；未验证不称通过。
- 未确认的 Git 提交/推送/发布先不执行；若后续已有发布授权由根收口，git push 必须 proxychains。

## Review Focus

1. 私有配置夹带其他同名 Context/user、相对文件或默认集群；启动/namespace/Context竞态使命令连错目标。
2. shell、前台 kubectl、后台子孙进程未清理，启动迟到泄漏、崩溃后配置遗留或清理误删其他会话。
3. PTY输入/输出无限积累、阻塞Tauri线程、Ctrl+C/resize只停UI不传终端，视图切换误重启命令。
4. Nacos CAS/历史身份/服务组实例属性错误导致覆盖新配置、修改其他目标、把healthy当enabled、丢失selector。
5. Nacos响应超时/权限/大体积/认证刷新导致写重放、把未核验写当成功，关闭或切目标后迟到结果覆盖草稿。

## Task 1: Nacos 后端收尾

**Files:** `src-tauri/src/nacos/{config,naming,http,mod,api_tests,tests}.rs`；必要 `connections.rs/lib.rs` 注册；`tests/fixtures/nacos_server.py`。
**Consumes:** 既有 `NacosService::runtime/begin_write`、`Runtime::request`，已写 API 回归。
**Produces:** `configs/config/publish/delete_config/history/history_config/rollback`；`services/service/update_service/update_instance` 与对应 `nacos_*` Tauri命令；DTO与 nacos-model 保持一致。

- [ ] 跑接手基线；记录当前缺失实现而非误称已完成。核对锁定 Nacos 2.3.2 官方控制器及实际自有服务，不能只用仿真正文决定协议。
- [ ] 运行已有配置/命名测试观察缺实现 RED；新增创建/历史合法回滚、权限/超时/未知写结果、CAS竞争/关闭和字段边界回归。
- [ ] 实现配置列表/详情 show=all、保留content/type/appName/desc，CAS现有md5，历史ID与完整target核对；删除、回滚单次请求并读回。
- [ ] 实现服务/实例查询和编辑；保留selector/cluster/ephemeral，不提交healthy；服务/实例快照校验及读回核验。
- [ ] 配置/列表≤4MiB、页≤200、内容≤1MiB、元数据≤256项/64KiB、15秒总期限；认证和写错误固定脱敏，未知结果不自动重发。
- [ ] 完整 Rust 模块与前端既有回归 GREEN；真实自有 Nacos 2.x 配置 CRUD/回滚和实例修改验证，记录未提供原子条件更新API的边界。

## Task 2: Nacos 页面与连接接线

**Files:** 新 `src/{NacosDialog,NacosPanel}.tsx`；`ConnectionManager.tsx`、`nacos-model.ts`、必要样式与 `tests/nacos-*.test.ts`/实际handler测试。
**Consumes:** Task1 DTO/命令、`profiles_test_nacos/profiles_connect_nacos`、既有安全存储。
**Produces:** Nacos 分类连接、可测试保存表单、配置/服务页与完整写确认/核验反馈。

- [ ] 先写实际handler/model RED：测试不保存、目标变化丢弃迟到结果、脏草稿切目标需确认、写开始后禁止重复/切换、失败保留草稿。
- [ ] 新增 Nacos 分类与工作区，匿名/密码、Keep/Replace/Remove、endpoint/namespace表单；引用既有系统凭据命令，不回填秘密。
- [ ] 紧凑 Namespace/Group/Data ID 搜索、分页、配置编辑/新建/发布/删除/历史对照/回滚；写确认包含完整target。
- [ ] 服务详情、实例启停/权重/metadata、保护阈值/服务metadata维护；成功显示服务端读回数据，超时显示需人工刷新确认。
- [ ] 完整 npm/TypeScript GREEN；私有桌面走真实IPC与自有Nacos，不把fake bridge视为验收。

## Task 3: 私有配置与本地 PTY 服务

**Files:** 新 `src-tauri/src/kubectl_terminal/{mod,target,process,supervisor,tests}.rs`；`main.rs/lib.rs` 与 `kubernetes.rs` 小范围钩子；独立依赖锁。
**Consumes:** Kubernetes 经过导入验证的 Kubeconfig、当前Context/generation和会话取消信号。
**Produces:** `terminal_open/input/resize/poll/namespace/close`，参数共同含sessionId/generation/terminalId；open含可选显式kubectlPath/shellPath及cols/rows，poll返回bytes/done/状态，不返回配置。

- [ ] 新测试先 RED：只保留指定Context及引用cluster/user、清空namespace为default、拒绝路径引用/认证插件，其他连接同名Context不串用。
- [ ] 新测试先 RED：真实PTY中文/输入/前台Ctrl+C/resize/stdout-stderr/长输出背压；旧generation与错session/终端ID拒绝、重复open与启动中关闭。
- [ ] 锁定官方 portable-pty；checked executable绝对路径，默认Linux bash无用户启动文件、关闭落盘history；Windows默认cmd /D、ConPTY，不自动下载kubectl。
- [ ] 私有配置/RC/锁文件与原子namespace写；fs2共享存活锁与受控遗留清理。只清理本应用识别的目录，保留其他活动会话。
- [ ] Linux本地监督子进程：subreaper/PDEATHSIG、信号处理与子孙进程回收；独立入口只启动已检查shell，参数和环境分离。Windows进程树/ACL按平台实现并记录实际验证范围。
- [ ] open预留名额，异步启动后重查代次/关闭意图，迟到进程精确回收；Input写线程、Reader背压、Poll分块，UI线程不阻塞。
- [ ] Context/select、workspace close、应用Exit会取消其终端，保留正常视图切换；完整模块 GREEN，进程树/权限/清理有实际OS证据。

## Task 4: K8s 前端终端与真实 kubectl 协议验证

**Files:** 新 `src/{KubeTerminal,terminal-model}.tsx/.ts`、`KubePanel.tsx`、`App.tsx`文案、样式和tests；`tests/fixtures/kubectl_api.py` 与桌面脚本。
**Consumes:** Task3命令、既有xterm，当前selection和desired namespace。
**Produces:** 资源/命令行切换、保持的终端、路径配置入口、准确默认目标状态。

- [ ] 真实handler/controller RED：视图隐藏仍保留终端、切Context重置且不自动打开、namespace变更不注入命令、迟到open/poll清理仅旧目标。
- [ ] 实现 xterm/fit、键盘输入/Tab/历史导航/Ctrl+C、复制粘贴、尺寸、连接/退出状态；poll须等xterm write回调再拉取。
- [ ] 模式切换不卸载运行终端；缺kubectl时说明路径并提供选择，默认target清楚标注，不宣称显式覆盖参数受限。
- [ ] 使用真实kubectl和自有HTTP/WebSocket Kubernetes API夹具测试 get/describe/logs-f/watch/exec-it、resize、RBAC拒绝与两连接隔离。fake输出仅用单元限界，不冒充kubectl协议。
- [ ] 实际私有桌面键入/中文/粘贴/模式切换、Context切换杀旧进程、图形资源页回归；各协议/GUI失败保留原始证据。

## Task 5: 整版验证、审查与 Linux 包

**Files:** 版本及锁、通用README、必要Linux打包输入；详细证据留机外。
**Consumes:** Tasks1–4 GREEN及实际服务/PTY/协议/桌面报告。
**Produces:** 同源0.1.9 Linux可运行制品、冻结manifest、明确Windows结果与残差。

- [ ] 唯一版本bump并校验package/Cargo/Tauri/锁；npm全套/tsc/Vite、Rust fmt/test --locked、release --locked；整应用含SSH/DB/K8s/MinIO/Nacos/PTY回归。
- [ ] 真实自有Nacos与真实kubectl夹具桌面整轮；确认无真实服务请求、无秘密诊断/命令history落盘，全部自己的进程/私有目录/显示器清理。
- [ ] Windows构建尝试与实机交互分别记录；构建不可用或旧问题带证据报告，不代称通过。
- [ ] 冻结输入/制品摘要交根唯一完整review；Important一次RED→GREEN及全套复测，原报告保持。
- [ ] 准备Linux可运行包与报告，明确已完成/未验收；未经根确认不替换用户运行制品、不提交/push/发布。


## Task 6: 续开发发现的终端输入完整性问题

- [x] 复现 namespace 更新和 Context 暂停导致已接受输入后缀被清空，两项回归先 RED。
- [x] 保留暂停队列，恢复时继续发送；已有输入发送结束后再更新默认 namespace；既有关闭、失败和旧 scope 隔离回归保持 GREEN。
- [x] 终端 12 项、前端 104 项、Rust 128 项、真实 Nacos 集成及同一独立审查席位复核通过。
- [x] 修复后 Linux 候选重建、真实 kubectl 协议与隔离桌面回归。
- [x] 更新同源候选摘要及本轮验证记录；保留 Windows 实机未验收边界。
