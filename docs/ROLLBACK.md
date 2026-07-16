# 回滚说明（M6）

## 1. 原则

- SQLite 为单主权威数据：回滚 = **恢复一致备份 + 回退二进制**，不是多主切换。
- 迁移只前进：无法自动“降级 schema”。若新版本迁移已写入，只能恢复迁移前备份。
- 先验证后切流量：`integrity` → `ready` → 业务冒烟。

## 2. 服务端二进制回滚

1. 停止服务。
2. 用上一版本包中的 `bin/mpgs-server`（及 `mpgs-dbtool`）覆盖当前二进制。
3. 保留数据目录与 env（除非同时做数据回滚）。
4. 启动服务。
5. 核对 `GET /v1/meta`：`service_version`、`build_git_sha` 与旧 `PROVENANCE.json` 一致；`schema_version` 与二进制期望一致。

若新版本已执行更高 schema 迁移而旧二进制不认识，**必须**同时恢复迁移前数据库备份。

## 3. 数据库回滚

```powershell
# 1) 停止写入（停服务）
# 2) 恢复
.\scripts\restore_db.ps1 -From D:\backups\mpgs-YYYYMMDDHHMM.db -To C:\ProgramData\MPGS\data\mpgs.db
# 或
mpgs-dbtool restore <backup> <dest>

# 3) 校验
mpgs-dbtool integrity <dest>
# 4) 启动服务并
# GET /health/ready
```

`restore` 会在目标路径不存在时写入并 migrate 到当前工具支持的最新版本。若要用旧二进制打开，请使用**与旧二进制匹配的 dbtool** 进行恢复，或使用备份文件本身已是旧 schema 且不做额外 migrate 的受控流程。

## 4. 客户端回滚

- Windows NSIS/MSI：安装上一版本安装包（未签名包仅限内测通道）。
- 客户端 SQLite 缓存与服务端权威库分离；回退客户端不会回退服务端目录。
- 未同步反馈队列位于客户端私有数据；升级/回退前可在设置中确认同步状态。

## 5. 配置回滚

- 环境文件与服务 unit/XML 纳入变更管理。
- 算法配置在数据库 `algorithm_configs` 中版本化；回退算法可切换 active 配置（需管理流程），不要手改历史迁移 SQL。

## 6. 决策记录

每次生产回滚记录：

- 时间、操作者、触发原因
- 回退的 `git_sha` / `service_version` / 备份文件名与 SHA-256
- 验证结果（ready、关键 API、错误率）
