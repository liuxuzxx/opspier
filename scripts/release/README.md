# Linux 发布

版本标签 `vMAJOR.MINOR.PATCH` 推送后，`Linux release` 工作流在 Ubuntu 22.04 x86_64 构建 AppImage、Debian 包，并发布到对应 GitHub Release 的 Assets：

- `OpsPier_<版本>_linux_x86_64.AppImage`
- `OpsPier_<版本>_linux_x86_64.deb`
- `SHA256SUMS-linux-x86_64.txt`

使用 GitHub 自动提供的 `GITHUB_TOKEN`，权限仅为 `contents: write`，无需个人令牌。构建、源码版本、包元数据或校验和检查失败时不会发布新 Release。新 Release 在三个附件均上传后才从草稿发布；已有公开 Release 的说明和其他平台附件保留，重跑替换同名 Linux 附件。

## 历史版本回补

先将工作流和 `scripts/release/`、`tests/release_helpers.test.mjs` 合入默认分支。无需修改、移动或重建旧标签。在 GitHub Actions → Linux release → Run workflow 选择默认分支，并填写 `tag`，例如 `v0.1.6`、`v0.1.7`、`v0.1.8`。

也可使用已授权的 GitHub CLI：

```bash
gh workflow run linux-release.yml --ref main -f tag=v0.1.6
gh workflow run linux-release.yml --ref main -f tag=v0.1.7
gh workflow run linux-release.yml --ref main -f tag=v0.1.8
```

工作流工具从执行该工作流的提交读取；应用另行 checkout 到标签解析出的精确提交。旧标签缺少 CI 文件、Tauri CLI 依赖和 `bundle.active=true` 均由外置工具和 override 解决，不把当前应用源码混入历史版本。分支名、提交 SHA、`refs/tags/...`、不存在的标签、版本不一致都会拒绝。回补创建的 Release 不设为 Latest。

必须检查实际 Actions 结果与目标 Release 的三个附件。静态检查通过不代表已完成构建或发布。如果仓库启用了不可变 Release，已发布历史 Release 无法增加附件；需仓库维护者按其发布策略处理。

## 下载与运行

打开对应版本的 GitHub Release，下载一个安装包及校验文件，在下载目录执行：

```bash
sha256sum --ignore-missing -c SHA256SUMS-linux-x86_64.txt
```

输出相应包的 `OK` 后，选择一种方式：

```bash
# AppImage：将 <版本> 替换为实际版本号。
chmod +x OpsPier_<版本>_linux_x86_64.AppImage
./OpsPier_<版本>_linux_x86_64.AppImage

# Debian/Ubuntu：由 apt 安装依赖和桌面入口。
sudo apt install ./OpsPier_<版本>_linux_x86_64.deb
opspier
```

目标是 x86_64 Linux 图形桌面，构建基线为 Ubuntu 22.04 / glibc 2.35；其他发行版仍需验证其 WebKitGTK 和桌面库兼容性。AppImage 需要 FUSE 2 支持（Ubuntu 22.04 为 `libfuse2`），也可尝试以 `--appimage-extract-and-run` 启动。凭据保存需要当前用户的 D-Bus 会话和已解锁的 Secret Service 提供程序，例如 GNOME Keyring；不要用 `sudo` 启动 GUI。

## 本地验证

```bash
node --test tests/release_helpers.test.mjs
bash -n scripts/release/publish-linux.sh
```

测试使用独立临时 Git 仓库、真实 `dpkg-deb` 测试包与 `sha256sum`，覆盖标签和源码版本绑定、缺包、重复包、架构、AppImage 文件头与校验和。实际应用构建还依赖标签中的两个锁文件、Rust stable、Node.js 24 和工作流声明的 Linux 开发库。Cargo 使用 `--locked`，不会为修复历史构建而自动升级依赖。

Rust SSH 协议测试还会启动标签中的 Python SSH 夹具。工作流使用 Python 3.12，将该标签的 `tests/fixtures/requirements.txt` 安装到运行器临时目录，通过 `AIC_OPS_FIXTURE_PYTHON_PATH` 传给夹具；不会依赖系统预装 Paramiko，也不会跳过协议测试。缺少依赖时会在安装或导入检查阶段直接失败，避免只出现夹具启动 EOF。

配置与 Actions 版本参照 [Tauri GitHub 发布指南](https://v2.tauri.app/distribute/pipelines/github/)、[tauri-action 配置](https://github.com/tauri-apps/tauri-action)及 [Linux 前置条件](https://v2.tauri.app/start/prerequisites/)。
