# OpsPier 工具箱移植设计：格式化工具与 HTTP 请求调试

## 意图

将 smart-agent 已有的“格式化工具”和“API 请求调试”页面移植到 OpsPier，保持 smart-agent 的页面结构、按钮语义、编辑区布局和结果展示方式，使 OpsPier 能在桌面端直接完成常用内容格式化和 HTTP 联调。

## 目标

- 提供 JSON、XML、TOML、YAML、SQL、Markdown 六种格式化类型。
- 每种格式支持格式化；适用类型支持压缩。
- 保留 smart-agent 的输入区、结果区、工具栏、复制和清空交互。
- 提供 GET、POST、PUT、PATCH、DELETE 请求。
- 支持 URL、请求 Header、请求 Body、发送中状态、响应状态码、响应 Header、响应 Body、请求耗时和复制响应。
- 请求失败、超时、非法 JSON 等情况显示可读错误。
- 工具页面作为 OpsPier 工作区独立标签，不修改 SSH、数据库、Kubernetes、Nacos、MinIO 的现有工作区行为。

## 非目标

- 不把请求记录保存到数据库或配置文件。
- 不自动保存 Header、Body、Token、Cookie 或密码。
- 不增加生产环境代理、重试、定时请求、批量请求、环境变量和脚本执行。
- 不修改 smart-agent 源码。
- 不复用原有连接工作区的状态或生命周期。

## 约束

- 页面视觉和交互以 smart-agent 当前实现为基线，不创造另一套工具页面风格。
- 保持 OpsPier 现有暗色主题、顶部标题栏和工作区标签结构。
- HTTP 请求必须由桌面端 Rust 侧发出，避免浏览器跨域限制；前端只负责编辑参数和展示结果。
- 请求 Header 解析按每行“名称: 值”处理，值中的冒号必须保留。
- HTTP 默认超时 10 秒；响应只在当前请求仍属于当前页面时写入界面。
- 格式化失败不能覆盖原输入。

## 参考实现

smart-agent：
- `web/src/pages/Toolbox/FormatTool/index.tsx`
- `web/src/pages/Toolbox/FormatTool/formatters.ts`
- `web/src/pages/Tools/Debug/ApiTester/index.tsx`
- `web/src/pages/Tools/Debug/ApiTester/httpMessage.ts`

## 交互设计

### 格式化工具

页面顶部使用格式类型切换、JSON 缩进选项、格式化、压缩、复制结果和清空按钮。下方左右两栏分别为“输入 <类型>”和“格式化结果”，编辑区行为和 smart-agent 对齐；格式化错误显示在工具栏下方。

### HTTP 请求调试

顶部使用方法选择、URL 输入和发送按钮。下方左侧为请求 Header 和请求 Body，右侧为响应内容和复制响应按钮。发送后显示“HTTP 状态码 + 耗时”；响应文本按 URL/Status、Header、Body 三段空行分隔。

## 实现判断

采用 smart-agent 的页面交互和视觉结构，按 OpsPier 的 React/Tauri 工程拆分组件。格式化逻辑迁移为本地前端工具模块；HTTP 请求由新的 Tauri 命令转发到 Rust `reqwest`，返回状态、Header、Body 和耗时。实现只新增工具页面、工具路由和对应测试，不改现有连接模块。

## 验收标准

- 六种格式分别可格式化；JSON、SQL、XML 等非法输入能显示错误且保留输入。
- JSON 支持 2/4 空格缩进；结果可复制；清空后输入和结果均为空。
- HTTP 请求可发送五种方法；Header 解析能保留 URL 或值中的冒号。
- HTTP JSON Body 非法时不发请求并提示错误；超时和网络错误有明确提示。
- 正常响应显示 URL、状态、Header、Body 和毫秒级耗时，响应可复制。
- 工具页面可与现有 SSH、数据库、Kubernetes、Nacos、MinIO 工作区并存。
- 前端测试、Rust 测试、构建和隔离桌面回归全部通过。

## 方案选择

已选择“按 smart-agent 页面和交互移植到 OpsPier”的方案。直接复用 OpsPier 现有连接工作区会把工具状态与连接生命周期耦合；单独 Web 页面又无法保持桌面工作区体验，因此不采用。
