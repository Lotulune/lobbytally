**[简体中文](README.md)** · [English](README.en.md)

<p align="center">
  <img src="web/public/app-icon-192.png" alt="LobbyTally 图标" width="88" height="88">
</p>

<h1 align="center">LobbyTally</h1>

<p align="center">和朋友一起，找到下一款联机游戏。</p>

<p align="center">
  <a href="https://mpgs.lunafleur.dpdns.org/">在线体验</a> ·
  <a href="#界面演示">界面演示</a> ·
  <a href="#本地运行">本地运行</a> ·
  <a href="https://github.com/Lotulune/lobbytally/issues">反馈问题</a>
</p>

LobbyTally 是面向熟人联机的 Steam 游戏发现与推荐工具。从新作到经典老游，结合游玩人数、合作偏好、私人房间和自建服务器等信息，帮助你和朋友缩小候选范围，再用“想玩”投票决定下一场玩什么。

浏览器可直接使用；仓库同时包含基于 Tauri 2 的桌面客户端。

## 界面演示

![LobbyTally 中文界面：近期正式发售游戏、推荐理由、联机人数、评价与在线人数](docs/images/lobbytally-feed-zh.png)

*中文界面示例，采用“极简白纸”主题。截图来自实际使用场景，展示时的游戏数据与部分界面细节可能与当前版本不同。中英文 README 共用这张中文截图。*

每张游戏卡片集中展示推荐顺序、联机模式、人数范围、评价、在线人数与数据可信度。推荐理由和待核实信息一起呈现，便于判断一款游戏是否适合自己的小队。

## 能做什么

| 功能 | 用途 |
| --- | --- |
| 四类游戏推荐 | 浏览近期正式发售、即将发售 / Demo、人气老游和经典老游。 |
| 偏好与反馈 | 设置常用人数、合作 / 竞技倾向等偏好，记录喜欢、不感兴趣和玩过 / 已拥有。 |
| 描述推荐 | 用自然语言说明需求；配置 AI 后可进一步分析候选并解释推荐理由，AI 不可用时回退基础推荐。 |
| 大家想玩 | 通过社区“想玩”投票了解玩家意向，为挑选游戏提供参考。 |
| 搜索与日历 | 搜索游戏、查看发售安排，再进入详情了解联机信息和相关证据。 |
| 主题与缓存 | 切换五套主题及特效强度；网络异常时可查看已缓存内容，待提交反馈保留在队列中。 |

## 怎么开始玩

1. 打开 [在线版](https://mpgs.lunafleur.dpdns.org/)，从四个推荐分区中选择一个。
2. 在“设置”里调整偏好，或在“描述推荐”中输入需求，例如：**“4 个人周末玩，合作为主，最好能开私人房间。”**
3. 比较推荐理由、人数与联机方式，给感兴趣的游戏点“想玩”，再到 Steam 商店确认详情。

浏览推荐无需绑定 Steam 账号。LobbyTally 账号与 Steam 账号相互独立，注册登录可使用账号相关功能。

### 推荐结果怎么看

- 推荐指数是排序参考，不是适合你的小队的概率，也不是 Steam 官方评分。
- 商店的“多人”标签不等于已确认支持私人房间、合作或自建服务器；资料不足时会保留未知或待核实提示。
- 价格、评价、在线人数和发售日期会变化，购买前请以 Steam 商店为准。
- 当前不提供 Steam 好友同步、游戏库交集或自动组队。更多边界见 [已知限制](docs/KNOWN_LIMITATIONS.md)。

## 本地运行

前置环境：**Rust 1.97+、Node.js 22、pnpm 9.15.9、Git**。下面的 PowerShell 示例启动一个带演示数据的本地环境，不需要 Steam 或 AI API Key。

### 1. 获取代码

```powershell
git clone https://github.com/Lotulune/lobbytally.git
cd lobbytally
pnpm install --frozen-lockfile
```

### 2. 启动 API

在仓库根目录的终端中运行：

```powershell
New-Item -ItemType Directory -Force data | Out-Null
$env:MPGS_DATABASE_PATH = '.\data\demo.db'
$env:MPGS_SEED_DEMO = 'true'
$env:MPGS_ADMIN_TOKEN = 'local-demo-only-token'
$env:MPGS_AI_PROVIDER = 'disabled'
cargo run -p mpgs-server --locked
```

API 默认监听 `http://127.0.0.1:17880`。上述令牌和演示数据仅用于本地开发；真实目录采集、AI 配置及生产凭据见 [开发指南](docs/DEVELOPMENT.md) 和 [运维手册](docs/OPERATIONS.md)。

### 3. 启动 Web

保持 API 运行，另开一个终端，在仓库根目录执行：

```powershell
pnpm web:dev
```

打开 [http://localhost:5173](http://localhost:5173)。Vite 会将 API 请求代理到本地服务端。

### 验证改动

```powershell
pnpm web:test
pnpm web:build
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

桌面开发需要额外的平台依赖，详见 [桌面 / Web 客户端指南](web/README.md)。自托管部署使用 Docker Compose、GHCR 镜像和 HTTPS 反向代理，详见 [运维手册](docs/OPERATIONS.md)。

## 技术与目录

前端使用 **React + TypeScript + Vite**，桌面外壳使用 **Tauri 2**；服务端使用 **Rust + Axum + Tokio**，以 **SQLite** 保存目录、推荐数据与反馈。基础推荐由确定性规则生成，AI 是可配置的增强能力。

| 路径 | 职责 |
| --- | --- |
| `web/` | 浏览器与桌面共用的前端、主题、缓存和反馈交互。 |
| `apps/server/` | HTTP API、会话与账号、推荐接口和任务调度。 |
| `apps/desktop/` | Tauri 桌面客户端。 |
| `apps/dbtool/` | 数据采集、审计、迁移和备份工具。 |
| `crates/` | 领域模型、推荐器、Steam 数据适配、存储与 AI。 |
| `migrations/` | SQLite 数据库迁移。 |
| `deploy/` | 容器配置、worker 与自动部署脚本。 |
| `docs/` | 产品、架构、开发与运维文档。 |

客户端通过 API 访问服务端；权威 SQLite 数据库与访问它的服务进程部署在同一台主机上，不通过网络共享数据库文件。

## 继续了解

| 文档 | 内容 |
| --- | --- |
| [开发指南](docs/DEVELOPMENT.md) | 环境准备、本地运行、采集与验证命令。 |
| [运维手册](docs/OPERATIONS.md) | 自托管、配置、备份、升级与回退。 |
| [HTTP API](docs/API.md) | 接口与数据契约。 |
| [系统架构](docs/ARCHITECTURE.md) | 组件职责与客户端 / 服务端边界。 |
| [推荐算法](docs/RECOMMENDATION.md) | 排序信号、推荐解释与校准。 |
| [AI 集成](docs/AI.md) | Provider、检索、配置与回退。 |
| [已知限制](docs/KNOWN_LIMITATIONS.md) | 当前的数据、产品与技术边界。 |
| [隐私说明](docs/PRIVACY.md) | 数据处理与隐私约定。 |

欢迎通过 [Issues](https://github.com/Lotulune/lobbytally/issues) 反馈体验、错误数据或功能建议。报告问题时，请附上复现步骤、使用平台与相关截图；不要提交密钥、访问令牌或个人数据库。
