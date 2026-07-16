# MPGS

MPGS（Multiplayer Game Scout）是一款面向熟人联机的 Steam 游戏发现工具。它优先推荐私人房间、合作模式、P2P 或可自建服务器的游戏，而不是简单复制 Steam 热门榜。

当前状态：`MVP 0.1 / M3+M4 关闭、M5 进行中` — 确定性推荐与公开 API、Tauri 桌面客户端已验收（见 [M4_CI_RUN](docs/M4_CI_RUN.md)）。M5 已落地 `mpgs-ai` Provider/Gateway、FTS/向量表与自然语言 AI 回退路径；真实 Provider 与语义检索仍在扩展中。
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
- [本地开发指南](docs/DEVELOPMENT.md)
- [系统架构](docs/ARCHITECTURE.md)
- [推荐算法](docs/RECOMMENDATION.md)
- [AI 检索与安全](docs/AI.md)
- [数据与存储](docs/DATA_STORAGE.md)
- [HTTP API 契约](docs/API.md)
- [MVP 开发计划](docs/MVP_PLAN.md)
- [M4 验收说明](docs/M4_ACCEPTANCE.md)
- [M4 CI 跨平台证据](docs/M4_CI_RUN.md)
- [M1 数据可行性报告](docs/M1_FEASIBILITY.md)
- [外部资料与核验记录](docs/SOURCES.md)

## 当前工程

```text
apps/server/              Axum 服务端（公开 API/OpenAPI、限流、管理覆盖、内部 jobs）
apps/dbtool/              SQLite migrate / Steam 候选采集 / audit / backup / restore CLI
crates/domain/            跨组件领域类型
crates/recommender/       确定性推荐核心
crates/steam-source/      Steam 源适配器 Spike（夹具 + 黄金集）
crates/storage/           SQLite Repository、迁移、ingest、jobs、备份
migrations/               SQLite 迁移脚本
scripts/                  备份/恢复辅助脚本
docs/                     产品与开发规格
```

验证命令：

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

运行服务端：

```powershell
# 可选：持久化 SQLite；未设置时使用内存库并自动种子演示目录
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
cargo run -p mpgs-dbtool --locked -- collect-steam-candidates .\data\m3-real.db 2000
cargo run -p mpgs-dbtool --locked -- m3-audit .\data\m3-real.db
```

采集使用经隔离的易变 Steam 商店搜索适配器，默认约每 1.1 秒一页并持久化续传游标；普通测试不会访问 Steam。`data/` 已忽略，不会把实时数据库提交到仓库。
