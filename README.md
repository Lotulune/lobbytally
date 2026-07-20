# MPGS

MPGS（Multiplayer Game Scout）是一款面向熟人联机的 Steam 游戏发现工具。它优先推荐私人房间、合作模式、P2P 或可自建服务器的游戏，而不是简单复制 Steam 热门榜。

当前状态：`MVP 0.2 / M7 本地工程验收` — 账号、社区、头像、AI 设置和显式演示数据策略已实现并完成本地验证，见 [M7_ACCEPTANCE](docs/M7_ACCEPTANCE.md)。真实 Steam 数据覆盖率、外部采集执行器、跨平台发布包、合规签字和代码签名仍是未关闭的发布门禁。M6 证据见 [M6_ACCEPTANCE](docs/M6_ACCEPTANCE.md)。
## MVP 能力

- 最近发售、即将发售/Demo、人气老游、经典老游四类推荐。
- 按常用人数、合作/竞技偏好、平台、预算、单次时长和自建服意愿筛选。
- 确定性推荐器生成基础分，AI 对小规模候选集二次分析并给出有证据的解释。
- SQLite 保存权威目录、评价/在线快照、推荐特征和用户反馈；客户端使用独立 SQLite 做离线缓存。
- 匿名可用，不要求用户在 MVP 阶段绑定 Steam 账号。

## 技术基线

- 服务端：Rust、Axum、Tokio、SQLite。
- 桌面客户端：Tauri 2、React、TypeScript。
- AI：服务端 Provider 抽象，兼容结构化输出、工具调用和 Embedding 的外部 API。
- 部署目标：Windows/Linux `x86_64` 与 `aarch64` 服务端；Windows/Linux/macOS 桌面客户端。

SQLite 数据文件只能由同机服务进程访问。远程客户端、采集节点和 AI Worker 必须通过 API 交互，不能打开共享数据库文件。

## 文档

- [产品需求文档](docs/PRD.md)
- [0.2 / M7 产品需求文档](docs/PRD_0.2.md)
- [0.3 / M8 AI 优化 PRD](docs/PRD_AI_0.3.md)
- [本地开发指南](docs/DEVELOPMENT.md)
- [系统架构](docs/ARCHITECTURE.md)
- [推荐算法](docs/RECOMMENDATION.md)
- [AI 检索与安全](docs/AI.md)
- [数据与存储](docs/DATA_STORAGE.md)
- [HTTP API 契约](docs/API.md)
- [MVP 开发计划](docs/MVP_PLAN.md)
- [M4 验收说明](docs/M4_ACCEPTANCE.md)
- [M4 CI 跨平台证据](docs/M4_CI_RUN.md)
- [M5 验收说明](docs/M5_ACCEPTANCE.md)
- [M6 验收说明](docs/M6_ACCEPTANCE.md)
- [M7 本地验收说明](docs/M7_ACCEPTANCE.md)
- [M7 本地验收运行记录](docs/M7_ACCEPTANCE_RUN.md)
- [运维手册](docs/OPERATIONS.md)
- [M1 数据可行性报告](docs/M1_FEASIBILITY.md)
- [外部资料与核验记录](docs/SOURCES.md)

## 当前工程

```text
apps/server/              Axum 服务端（公开 API/OpenAPI、限流、管理覆盖、内部 jobs）
apps/dbtool/              SQLite migrate / Steam 候选采集 / audit / backup / restore CLI
apps/desktop/             Tauri 2 桌面壳
crates/domain/            跨组件领域类型
crates/recommender/       确定性推荐核心
crates/steam-source/      Steam 源适配器 Spike（夹具 + 黄金集）
crates/storage/           SQLite Repository、迁移、ingest、jobs、备份
crates/ai/                AI Provider / Gateway / 校验
packaging/                Linux systemd / Windows WinSW 服务布局
migrations/               SQLite 迁移脚本
scripts/                  备份/恢复/M4–M6 验收与打包
docs/                     产品与开发规格
```

验证命令：

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Docker 部署同时提供 Web、API 和持续 Steam worker：

```bash
cp deploy/mpgs.env.example deploy/mpgs.env
# 填写随机 MPGS_ADMIN_TOKEN；Steam AppList 增量同步还需可选的服务端 Web API Key。
docker compose -f deploy/docker-compose.yml up -d --build
curl http://127.0.0.1:18082/health/ready
```

Compose 只把 Web 网关发布到宿主机 `127.0.0.1:18082`，API 与 SQLite 保留在内部网络/绑定卷中。生产域名应由宿主机 Nginx 或其他 TLS 反向代理转发到该端口。

`main` 分支更新后，GitHub Actions 会构建 release 镜像并发布到 GHCR。VPS 首次切换到远端镜像时复制 `deploy/.env.example` 为 `deploy/.env`；以后运行 `deploy/update.sh` 即可执行 `git pull`、镜像拉取、无本地构建滚动更新和健康检查。生产 VPS 使用 `deploy/mpgs-update.timer` 每 5 分钟自动检查镜像更新。

运行服务端：

```powershell
# 可选：持久化 SQLite；演示目录仅在显式设置 MPGS_SEED_DEMO=true 时加载
New-Item -ItemType Directory -Force data | Out-Null
$env:MPGS_DATABASE_PATH = '.\data\mpgs.db'
$env:MPGS_SEED_DEMO = 'true' # 仅本地演示；持久化空库默认不写入虚构数据
$env:MPGS_ADMIN_TOKEN = 'dev-only-token'
cargo run -p mpgs-server
```

默认监听 `127.0.0.1:8080`。公开端点示例：

```powershell
Invoke-RestMethod 'http://127.0.0.1:8080/v1/meta'
Invoke-RestMethod 'http://127.0.0.1:8080/openapi.json'
Invoke-RestMethod 'http://127.0.0.1:8080/v1/feeds/classic_legacy?limit=5'
Invoke-RestMethod 'http://127.0.0.1:8080/v1/search?q=Deep'
$session = Invoke-RestMethod -Method Post 'http://127.0.0.1:8080/v1/session/anonymous'
```

真实目录的 M3 数据门禁：

```powershell
$env:MPGS_STEAM_WEB_API_KEY = '<server-side Steam Web API key>'
cargo run -p mpgs-dbtool --locked -- collect-steam-catalog .\data\m3-real.db 1 1000
cargo run -p mpgs-dbtool --locked -- collect-steam-candidates .\data\m3-real.db 2000
cargo run -p mpgs-dbtool --locked -- m3-audit .\data\m3-real.db
```

官方目录同步与候选发现分离：`collect-steam-catalog` 使用服务端 Web API Key 和持久化增量游标，`collect-steam-candidates` 使用经隔离的易变 Steam 商店搜索适配器，默认约每 1.1 秒一页。服务端调度器入队后，由同机 `run-steam-worker-once` 领取作业并回写运行状态。普通测试不会访问 Steam；`data/` 已忽略，不会把实时数据库提交到仓库。
