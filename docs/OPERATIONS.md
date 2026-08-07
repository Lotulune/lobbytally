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
# 服务端布局（含已与二进制 --build-info 核对的 PROVENANCE.json + SHA256SUMS）
.\scripts\package_server.ps1

# 桌面（未签名；CI 已有三平台 smoke）
pnpm exec tauri build --config apps/desktop/src-tauri/tauri.conf.json --ci --no-sign -b nsis
```

发布前核对 `PROVENANCE.json` 中的 `service_version`、`git_sha`、`schema_version`、`algorithm_version` 与 `signing`。

### 2.2 Linux（systemd）

```bash
# 解压 package 后
sudo bash ./linux/install.sh .
# 编辑 /etc/mpgs/mpgs.env：MPGS_DATABASE_PATH、MPGS_ADMIN_TOKEN
sudo -u mpgs mpgs-dbtool migrate /var/lib/mpgs/mpgs.db
sudo systemctl start mpgs-server
curl -sS http://127.0.0.1:17880/health/ready
```

### 2.3 Windows（WinSW）

1. 使用 `package_server.ps1` 生成布局。
2. 将 WinSW 可执行文件放到 `windows\winsw.exe`。
3. 管理员 PowerShell 中安全输入管理 Token 并安装：

```powershell
$adminToken = Read-Host 'MPGS admin token' -AsSecureString
.\windows\install-service.ps1 -PackageRoot . -AdminToken $adminToken -Start
```

安装器把只读程序复制到 `%ProgramFiles%\MPGS`，把数据库/日志放到
`%ProgramData%\MPGS`，收紧 ACL，并以 `LocalService` 运行。未传 `-Start`
时只安装 Manual 服务且不启动；不得直接从下载目录或用户可写目录运行服务。
其他 Steam/AI 环境变量应由管理员加入受保护的已安装服务 XML，随后重启服务。

验证：`Invoke-RestMethod http://127.0.0.1:8080/v1/meta`

卸载：

```powershell
& "$env:ProgramFiles\MPGS\windows\uninstall-service.ps1" `
  -PackageRoot "$env:ProgramFiles\MPGS"
```

### 2.4 反向代理

服务默认只监听本机。对外暴露时在前面放置 TLS 终止代理，并仅在入口清洗转发头后才设置 `MPGS_TRUST_PROXY_HEADERS=true`。Docker 后端模式使用 `deploy/mpgs-api-host.nginx.conf` 反代到 `127.0.0.1:18081`；完整模式使用 `deploy/mpgs-host.nginx.conf` 反代到 Web 网关 `127.0.0.1:18082`。

## 3. 日常运维

### 3.1 健康

- `GET /health/live`：进程存活。
- `GET /health/ready`：迁移版本 + 数据库可读 + 最小目录就绪。
- `GET /.well-known/mpgs`：客户端连接发现，不依赖数据库状态。
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

```powershell
mpgs-dbtool migrate <db>
$env:MPGS_STEAM_WEB_API_KEY = '<server-side Steam Web API key>'
mpgs-dbtool collect-steam-catalog <db> 1 1000
mpgs-dbtool collect-steam-candidates <db> 2000
mpgs-dbtool enrich-steam-app <db> <app-id>
mpgs-dbtool enrich-steam-candidates <db> 100
$env:MPGS_STEAM_WORKER_ID = 'mpgs-steam-worker-1'
mpgs-dbtool run-steam-worker-once <db> 1 100
mpgs-dbtool import-golden-profiles <db>
mpgs-dbtool m3-audit <db>
mpgs-dbtool m7-data-audit <db>
mpgs-dbtool recommendation-audit <db> --as-of 2026-08-02 --top 20 --strict
mpgs-dbtool recommendation-golden-evaluate <labels.json> --json
mpgs-dbtool sync-retrieval <db>
mpgs-dbtool extract-offline-features <db>
mpgs-dbtool embed-documents <db>
```

`collect-steam-catalog` 只读取服务端环境中的 `MPGS_STEAM_WEB_API_KEY`，密钥不得作为命令参数、写入 SQLite 或进入客户端包。服务端以 5 分钟的默认检查频率观察三类独立到期时间：目录同步 15 分钟、候选发现 6 小时、富化 5 分钟。候选发现同时刷新近期发售与 `comingsoon` 通道；候选池未达目标时，每个通道默认推进 10 页，可用 `MPGS_CANDIDATE_WORKER_PAGES` 在 `1..100` 内覆盖。候选池达标后，环境变量不再放大稳态扫描，每个通道刷新首页并最多推进 1 个续页。每类任务在同类 `pending` 或 `leased` 作业存在时不会再入队，因此慢目录同步不会积压并抢占候选/富化。使用同一数据库文件的主机必须周期执行 `run-steam-worker-once`。worker 以 SQLite 租约防止重复领取，并把成功时间、下次运行、错误类别、游标和覆盖率回写到 `/admin/v1/data-status`。`enrich-steam-app` 是运营点名 AppID 的受控强制商店入库路径。不要通过网络文件系统运行该 worker；远程部署需要走受控 ingestion API。默认商店区域 `CN/schinese`。富化会优先补齐影响日历的商店详情，再同步全语言评价汇总、简体中文热门评价前 10 条与 CCU；`MPGS_ENRICH_STORE_ONLY` / `MPGS_ENRICH_SKIP_*` 在 CLI 与后台 worker 中语义一致。采集需遵守限流与 [SOURCES.md](SOURCES.md)。

日历接口的 `upcoming` 状态默认只查询今天至未来 60 天；`recent` 仍查询过去 180 天至今天。`upcoming` 的未知日期分区最多返回最近更新的 100 条，避免历史候选把日历响应放大。需要更大窗口时必须显式传入 `from` / `to`，避免前端或缓存配置把日历默认范围扩大到 180 天。

`m7-data-audit` 是 DATA-206 的发布前命令，默认严格验证：至少 2,000 个规范化候选、300 个可信熟人联机画像、四个分区各 20 个候选、日期与封面各 95% 覆盖，以及 300 个重点画像各自连续 7 天的评价和 CCU 数据。它使用当前算法配置及与公开 feed 相同的分区资格规则，失败会返回非零退出码。若 Steam 当前新游确实不足 20 个，只能显式记录原因后运行：

```powershell
mpgs-dbtool m7-data-audit <db> --allow-upcoming-shortfall='官方目录当日新游不足'
```

该例外只豁免 `upcoming` 分区，不能绕过其他数据门禁；建议将命令输出连同原因保存到发布记录。

`recommendation-audit` 以 SQLite `read_only/query_only` 模式重放四分区推荐。`--as-of` 必填，`--user-id` 可选，`--top` 仅控制报告摘要；确定性质量门禁始终使用 Top20。发布门禁应加 `--strict`，这样裁剪率、指数区分度/分桶、跨证据向量同分、可行模式占比、MMR 倒置理由或证据 ID 解析失败时进程返回非零。`not_applicable` 不算失败，也不能当作通过。该命令不迁移或修改数据库，也不评估缺少人工标签/归因结果时的 NDCG 与校准质量。

`recommendation-golden-evaluate` 不读取数据库，只读取 `recommendation_golden_labels_v1` JSON。文件必须至少有 200 条唯一 persona/game/section 判断以及至少 5 个 persona、5 个游戏；所有特征和 CCU/评论基线必须显式归一到 `0..1`，人工相关性为 `0..3`。工具输出分区 pairwise logistic 候选权重、persona/game 双五折指标和 `freeze_eligible`，但不写 `algorithm_configs`。`freeze_eligible=false` 不会单独造成非零退出，CI 必须解析 `--json` 结果；完整 Schema 与门槛见 [推荐算法规格](RECOMMENDATION.md#131-黄金测试集)。

### 3.5 Docker / Compose

`deploy/docker-compose.yml` 包含 `mpgs-server`、可选静态 Web 网关 `mpgs-web` 和周期执行租约任务的 `mpgs-worker`。SQLite 与头像通过同一宿主机目录挂载到 `/var/lib/mpgs`，不得改成网络共享卷。
一次性的 `mpgs-init` 会在 server/worker 启动前创建并校正该目录的容器
UID/GID（不会递归扫描备份目录）；worker 还通过 `.worker-health` 暴露最近一次运行状态，连续失败会退出并由
Compose 重启。`docker compose ps` 出现 `unhealthy` 时应检查 worker 日志，而不能只看
API readiness。

复制环境模板后，在 `deploy/.env` 选择部署模式：

```dotenv
# 只运行 API + worker，通过 127.0.0.1:18081 反代
MPGS_DEPLOY_MODE=backend

# 或运行 API + worker + Web UI，通过 127.0.0.1:18082 反代
MPGS_DEPLOY_MODE=full
```

生产默认保留 `full` 以兼容现有站点。只面向桌面客户端的新安装建议使用 `backend`。切换模式后执行 `deploy/update.sh`；脚本会停止全部旧入口，只拉取并启动该模式所需服务，并在切到 `backend` 时显式移除旧 Web 容器。

```bash
cp deploy/mpgs.env.example deploy/mpgs.env
mkdir -p deploy/runtime
chmod 600 deploy/mpgs.env
docker compose -f deploy/docker-compose.yml up -d --build
docker compose -f deploy/docker-compose.yml exec mpgs-server \
  mpgs-dbtool integrity /var/lib/mpgs/mpgs.db
```

直接使用 Compose 开发构建时，可显式选择服务：

```bash
# 后端，不启动 mpgs-web
docker compose -f deploy/docker-compose.yml up -d --build mpgs-server mpgs-worker
curl http://127.0.0.1:18081/.well-known/mpgs

# 完整安装
docker compose -f deploy/docker-compose.yml up -d --build
curl http://127.0.0.1:18082/.well-known/mpgs
```

迁移已有数据库时，先使用 `mpgs-dbtool backup <source> <backup>` 生成一致性副本，再把副本放到 `deploy/runtime/mpgs.db`。worker 默认每 60 秒领取一个任务；单轮富化默认 20 个 App，以免 30 分钟租约在串行任务中失效。没有 `MPGS_STEAM_WEB_API_KEY` 时官方 AppList 同步保持禁用，但候选发现、商店详情、评价和 CCU 富化仍会执行。连续 7 天采集完成前，`m7-data-audit` 返回失败属于预期状态。

正式 VPS 不应在宿主机编译 Rust。CI 的 Web、Rust quality 与 Linux package
门禁全部通过后，`.github/workflows/container-images.yml` 才会发布成对的
`mpgs-server:sha-<commit>` / `mpgs-web:sha-<commit>`。两个不可变镜像均存在后，
工作流最后才移动 `mpgs-server:release-main` 指针；VPS 从该指针读取同一个 commit，
不会混用两个发布版本。VPS 初始化：

```bash
cp deploy/.env.example deploy/.env
chmod 600 deploy/.env deploy/mpgs.env
# 私有 GHCR 包需要先使用具备 read:packages 的 PAT 登录；公开包无需登录。
docker login ghcr.io
./deploy/update.sh
```

`deploy/update.sh` 默认读取 `MPGS_RELEASE_POINTER_IMAGE`；它拒绝脏工作树，且只有
当指针 SHA 与远端部署分支 tip 一致时才继续。脚本先从目标提交提取临时 Compose
配置并拉取两个对应的 `sha-*` 镜像；镜像、数据库与健康检查全部成功后才把源码
fast-forward 到同一 SHA。更新器从临时副本运行，因此源码切换不会替换执行中的
脚本；旧、新 Compose 快照共用显式项目名但分开执行，失败时源码仍停留在旧提交，
旧镜像也只由旧 Compose 配置恢复。切换前脚本停止 Web/worker/server 写入，使用
**当前旧镜像**中的 `mpgs-dbtool`
生成并验证 `deploy/runtime/backups/pre-update-*.db`。新版本必须同时通过数据库
完整性、readiness、`/v1/meta.build_git_sha` 和 worker 健康检查；任一失败时，脚本会
保留失败数据库副本、恢复升级前备份，并把旧容器的精确本地 image ID 临时标记为
标准回滚镜像引用后自动重启，避免依赖已经移动的旧 tag。

成功部署后更新器默认只保留最近 3 份 `pre-update-*.db`，可在 `deploy/.env` 中用
`MPGS_BACKUP_RETENTION_COUNT` 调整（范围 `1..100`）。`MPGS_DEPLOY_HEALTH_TIMEOUT_SECS`
默认 600 秒，用于低配主机上的一次性迁移/索引构建；它不会改变 systemd 的总超时。

紧急固定某个已发布版本时，可在 `deploy/.env` 临时设置完整的 40 位
`MPGS_RELEASE_SHA`；成功恢复并确认后应删除该 pin，重新跟随发布指针。运行时密钥只
保存在 `deploy/mpgs.env`，不得放入 `deploy/.env`、GitHub workflow 或镜像标签。
`.dockerignore` 明确排除了两个部署 env 与 `deploy/runtime`，但这些文件仍应保持
`0600` 且绝不能手工加入构建上下文。

内置 AI 使用 OpenAI-compatible Provider 时配置 `MPGS_AI_PROVIDER=openai_compat`、`MPGS_AI_BASE_URL`、`MPGS_AI_API_KEY`、`MPGS_AI_MODEL` 和 `MPGS_AI_TIMEOUT_SECS`。Embedding 默认使用本地 `hash`，只有确认上游提供兼容 Embedding 接口时才切换为 `openai_compat`。AI 失败会回退到确定性推荐；更换模型后需重启 `mpgs-server`，并应先用 `/v1/meta` 和一条自然语言推荐请求验证 `ai_available`、`ai_status` 与延迟。

VPS 使用仓库提供的 systemd 单元主动检查更新，无需向 GitHub 上传 VPS SSH 私钥。当前部署目录为 `/home/ubuntu/mpgs/src`；如果目录或运行用户不同，先修改 `deploy/mpgs-update.service`。安装后会立即执行一次，并在每次任务结束 5 分钟后再次检查：

```bash
sudo install -m 0644 deploy/mpgs-update.service deploy/mpgs-update.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now mpgs-update.timer
sudo systemctl start mpgs-update.service
systemctl status mpgs-update.timer --no-pager
```

大库的在线备份、外键检查和完整性检查可能超过 10 分钟，仓库单元预留 30 分钟；若服务因旧版超时进入 `failed` 且 timer 不再活动，更新单元后需重新执行 `daemon-reload` 与 `enable --now`。

定时器只在成对发布指针前移后部署；指针未变化时 Compose 不重建容器。失败与自动
回滚记录在 `journalctl -u mpgs-update.service`；失败数据库副本不会自动删除，升级前
备份按上述保留策略清理，应持续监控磁盘容量。

### 3.6 密钥轮换

1. 生成新 `MPGS_ADMIN_TOKEN`。
2. 更新环境文件 / 服务配置。
3. 滚动重启 `mpgs-server`。
4. 使旧 Token 立即失效（进程内只读启动时环境）。

Steam/AI Key 只放在服务端环境；客户端包与日志不得包含。

## 4. 升级

1. 备份数据库与当前 `PROVENANCE.json`。
2. 停止服务（systemd `stop` / WinSW `stop`）。
3. 替换二进制与文档；保留数据目录与 env。
4. `mpgs-dbtool migrate <db>`（或启动时自动 migrate）。当前最新为 `0021_read_path_indexes`，它在 `0020_feed_query_indexes` 的基础上，为 Feed/日历的最新评论读取和 demo/playtest 反查补充索引。
5. 启动并检查 `/health/ready` 与 `/v1/meta` 的 `schema_version`。
6. 冒烟：四分区、搜索、详情、偏好、反馈、NL fallback。

不可逆迁移须在发布说明中标记。当前迁移只前进不回退。

## 5. 日志与隐私

- 使用 `RUST_LOG`（默认 info）。
- 禁止记录 API Key、Bearer、完整 AI Prompt、私人原文。
- 请求关联使用 `x-request-id`。

## 6. 已知限制

见 [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md)。
