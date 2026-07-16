# 签名与自动更新策略（M6）

本文记录 **MVP 0.1 的明确策略**。实际启用代码签名、公证与更新通道需要发布负责人单独授权（证书采购、私钥托管、发布 CI），见 [MVP_PLAN.md §8](MVP_PLAN.md#8-需要后续明确授权的工作)。

## 1. 当前状态（工程默认）

| 产物 | 签名 | 自动更新 |
| --- | --- | --- |
| `mpgs-server` / `mpgs-dbtool` | **未签名**（`PROVENANCE.json` → `signing: unsigned`） | 人工替换二进制 + 备份回滚 |
| Windows NSIS/MSI | CI/`--no-sign` 冒烟 | **未启用** Tauri updater |
| Linux DEB | 未仓库签名 | 包管理器/人工 |
| macOS APP | 未公证（需 Apple 开发者账号） | **未启用** |

CI 桌面构建显式使用 `--no-sign`，避免在无证书环境失败。

## 2. 服务端可追溯性

`scripts/package_server.ps1` 生成：

- `PROVENANCE.json`：`service_version`、`git_sha`、`built_at_utc`、`schema_version`、`algorithm_version`、`signing`
- `SHA256SUMS.txt`：包内文件摘要
- 编译期 `MPGS_BUILD_GIT_SHA` → `GET /v1/meta.build_git_sha`

发布清单必须同时归档上述文件与迁移版本号。

## 3. 目标签名策略（授权后）

### 3.1 Windows

- 使用组织代码签名证书（OV/EV）对 `mpgs-server.exe`、`mpgs-dbtool.exe` 与 NSIS/MSI 签名。
- 私钥放在 HSM 或 CI OIDC 短期证书，不入库。
- 发布流水线在签名后重算 SHA-256 并写入 GitHub Release / 内部制品库。

### 3.2 macOS

- Developer ID Application 签名 + notarization。
- 仅在 macOS runner 上执行；禁止在非 Apple 主机伪造正式签名产物。

### 3.3 Linux

- 可选：对 DEB 使用维护者 GPG；服务器二进制至少提供 SHA-256 与 git 标签对应关系。

## 4. 桌面自动更新（目标设计）

采用 Tauri 2 updater 插件（**尚未接入依赖**，避免无公钥构建）：

1. 生成更新密钥对；**公钥**写入 `tauri.conf.json`，**私钥**仅发布管道可见。
2. 发布静态 `latest.json` + 分平台安装包 URL（HTTPS）。
3. 客户端仅校验签名后的更新清单；失败时保留当前版本并提示手动下载。
4. 内测通道与正式通道分离 endpoint。

在未完成第 1–3 步前，桌面分发以安装包全量替换为准。

## 5. 安全约束

- 客户端包与更新通道不得包含 Steam/AI/管理 Key。
- 更新 URL 仅 HTTPS；禁止明文回落。
- 签名失败 = 拒绝安装/更新，不得“跳过校验”默认开启。

## 6. 发布检查表（签名项）

- [ ] 证书与 Apple/Windows 账号已由负责人采购并授权 CI
- [ ] 公钥已嵌入客户端；私钥轮换演练完成
- [ ] 每个发布物有版本 + git SHA + 算法版本 + schema + 摘要
- [ ] 签名策略变更已更新本文日期与签字

| 项目 | 内容 |
| --- | --- |
| 策略版本 | 2026-07-17 m6-unsigned-baseline |
| 发布负责人确认 | _待填_ |
