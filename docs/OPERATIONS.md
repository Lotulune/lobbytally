# MPGS 运维手册（M6）

面向单节点 MVP：一个 `mpgs-server` 进程 + 本机 SQLite。数据库文件不得放在网络共享或同步盘上。

## 1. 组件

| 组件 | 产物 | 职责 |
| --- | --- | --- |
| `mpgs-server` | 服务端二进制 | 公开 API、推荐、AI 网关、管理/内部 jobs |
| `mpgs-dbtool` | 运维 CLI | migrate / integrity / backup / restore / 采集与检索同步 |
| 桌面客户端 | Tauri NSIS/DEB/APP | 匿名浏览、离线缓存；不持有服务端 Key |

## 2. 安装

### 2.1 打包

```powershell
# 服务端布局（含 PROVENANCE.json + SHA256SUMS）
.\scripts\package_server.ps1

# 桌面（未签名；CI 已有三平台 smoke）
pnpm exec tauri build --config apps/desktop/src-tauri/tauri.conf.json --ci --no-sign -b nsis
```

发布前核对 `PROVENANCE.json` 中的 `service_version`、`git_sha`、`schema_version`、`algorithm_version` 与 `signing`。

### 2.2 Linux（systemd）

```bash
# 解压 package 后
sudo ./linux/install.sh .
# 编辑 /etc/mpgs/mpgs.env：MPGS_DATABASE_PATH、MPGS_ADMIN_TOKEN
sudo -u mpgs mpgs-dbtool migrate /var/lib/mpgs/mpgs.db
sudo systemctl start mpgs-server
curl -sS http://127.0.0.1:8080/health/ready
```

### 2.3 Windows（WinSW）

1. 使用 `package_server.ps1` 生成布局。
2. 将 WinSW 可执行文件放到 `windows\winsw.exe`。
3. 管理员 PowerShell：`.\windows\install-service.ps1 -PackageRoot .`
4. 在服务 XML 或主机环境中配置 `MPGS_ADMIN_TOKEN` 与数据库路径。
5. 验证：`Invoke-RestMethod http://127.0.0.1:8080/v1/meta`

卸载：`.\windows\uninstall-service.ps1 -PackageRoot .`

### 2.4 反向代理

服务默认只监听本机。对外暴露时在前面放置 TLS 终止代理，并仅在入口清洗转发头后才设置 `MPGS_TRUST_PROXY_HEADERS=true`。

## 3. 日常运维

### 3.1 健康

- `GET /health/live`：进程存活。
- `GET /health/ready`：迁移版本 + 数据库可读 + 最小目录就绪。
- `GET /v1/meta`：版本、算法配置、schema、build SHA、数据新鲜度。

### 3.2 备份

```powershell
.\scripts\backup_db.ps1 -DbPath C:\ProgramData\MPGS\data\mpgs.db -OutPath D:\backups\mpgs-$(Get-Date -Format yyyyMMddHHmm).db
# 或
mpgs-dbtool backup <db> <backup-path>
```

使用 Online Backup API（`mpgs-dbtool backup`），不要复制活动中的 `-wal`/`-shm` 组合。

### 3.3 恢复

见 [ROLLBACK.md](ROLLBACK.md)。恢复后必须 `integrity` + `ready` 通过再切流量。

### 3.4 数据富化与检索

```text
mpgs-dbtool migrate <db>
mpgs-dbtool collect-steam-candidates <db> 2000
mpgs-dbtool enrich-steam-candidates <db> 100
mpgs-dbtool import-golden-profiles <db>
mpgs-dbtool m3-audit <db>
mpgs-dbtool sync-retrieval <db>
mpgs-dbtool extract-offline-features <db>
mpgs-dbtool embed-documents <db>
```

默认商店区域 `CN/schinese`。采集需遵守限流与 [SOURCES.md](SOURCES.md)。

### 3.5 密钥轮换

1. 生成新 `MPGS_ADMIN_TOKEN`。
2. 更新环境文件 / 服务配置。
3. 滚动重启 `mpgs-server`。
4. 使旧 Token 立即失效（进程内只读启动时环境）。

Steam/AI Key 只放在服务端环境；客户端包与日志不得包含。

## 4. 升级

1. 备份数据库与当前 `PROVENANCE.json`。
2. 停止服务（systemd `stop` / WinSW `stop`）。
3. 替换二进制与文档；保留数据目录与 env。
4. `mpgs-dbtool migrate <db>`（或启动时自动 migrate）。
5. 启动并检查 `/health/ready` 与 `/v1/meta` 的 `schema_version`。
6. 冒烟：四分区、搜索、详情、偏好、反馈、NL fallback。

不可逆迁移须在发布说明中标记。当前迁移只前进不回退。

## 5. 日志与隐私

- 使用 `RUST_LOG`（默认 info）。
- 禁止记录 API Key、Bearer、完整 AI Prompt、私人原文。
- 请求关联使用 `x-request-id`。

## 6. 已知限制

见 [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md)。
