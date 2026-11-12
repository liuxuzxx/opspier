# OpsPier

桌面运维工具，集中管理远程终端、数据库和 Kubernetes 集群。

## 技术栈

- 桌面框架：Tauri 2、Rust。
- 界面：React 19、TypeScript、Ant Design 6、Vite。
- 终端：xterm.js、SSH2。
- 数据库：SQLx、SQLParser。
- Kubernetes：kube-rs、k8s-openapi。
- 凭据保存：操作系统安全凭据库，目前支持 Linux Secret Service。

## 已有功能

- 连接分类管理、连接测试和状态展示；连接列表支持收起与展开。
- SSH 密码及私钥认证、主机指纹校验、终端分屏和复制粘贴。
- MySQL / Apache Doris 库表浏览、字段与索引查看、只读 SQL、分页、查询取消和 CSV 导出。新建连接自动协商 TLS，默认数据库可选。
- Kubernetes 配置文件导入及粘贴、Context 和命名空间选择、资源搜索、后台刷新、CRD、YAML 和 Pod 日志查看。
- 使用系统凭据库保存密码和认证材料，重开连接时无需重复输入。

## 计划功能

- GitHub Actions 自动构建 Linux 和 Windows 安装包，提供版本下载。
- Windows 兼容适配及安全凭据保存。
- 增加 Oracle、SQL Server、SQLite 和 Elasticsearch 支持。
- 增加 MinIO、RustFS、NFS 等存储服务管理。
- 持续完善连接诊断、资源检索和工作区交互。
