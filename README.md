**[简体中文](README.md)** · [English](README.en.md)

<p align="center">
  <img src="web/public/app-icon-192.png" alt="LobbyTally 图标" width="88" height="88">
</p>

<h1 align="center">LobbyTally</h1>

<p align="center">和朋友一起，找到下一款联机游戏。</p>

<p align="center">
  <a href="https://mpgs.lunafleur.dpdns.org/">在线体验</a> ·
  <a href="#界面演示">界面演示</a> ·
  <a href="docs/VERCEL_SUPABASE.md">云端部署</a> ·
  <a href="#源码运行">源码运行</a> ·
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

## Vercel + Supabase 部署

当前部署将 **React / Vite 前端和 Rust / Axum API 放在同一个 Vercel 容器服务中**，由 [Dockerfile.vercel](Dockerfile.vercel) 构建，[vercel.json](vercel.json) 指定东京区域 `hnd1`。Supabase 提供 PostgreSQL、私有头像存储和 Cron；服务端持久状态保存在云端，不需要 VPS 或常驻 worker。

| 部分 | 当前实现 |
| --- | --- |
| 数据库 | Supabase PostgreSQL 的私有 `mpgs` schema；服务端通过端口 `5432` 的 Session pooler 连接。 |
| 账号 | 应用自己的 Rust 账号与会话实现，无需配置 Supabase Auth。 |
| 头像 | Supabase Storage 的私有 `avatars` bucket，由 API 提供头像访问。 |
| 采集与维护 | Supabase Cron 每分钟触发有限时长的维护批次，每 5 分钟触发头像清理；游标与租约保存在 PostgreSQL。 |
| AI | 可选增强，默认 `MPGS_AI_PROVIDER=disabled`，基础推荐仍可使用。 |

部署准备：Git、Node.js 22、Supabase 和 Vercel 账号，以及本仓库的本地克隆。以下命令均在仓库根目录执行；Rust 和前端构建由 Vercel 容器完成。

### 1. 初始化新的 Supabase 项目

创建 Supabase 项目，区域尽量靠近 Vercel 的 `hnd1`。从仓库根目录使用已验证的 CLI 版本关联项目并应用迁移：

```powershell
npx --yes supabase@2.121.0 login
npx --yes supabase@2.121.0 init
npx --yes supabase@2.121.0 link --project-ref YOUR_PROJECT_REF
npx --yes supabase@2.121.0 db push --linked --skip-vault --dry-run
npx --yes supabase@2.121.0 db push --linked --skip-vault
npx --yes supabase@2.121.0 migration list --linked
```

`init` 用于尚无本地 `supabase/config.toml` 的首次设置。迁移会创建空目录、运行默认值、头像 bucket、调度函数，以及供后台连接的 `lobbytally_runtime` 角色；该角色的随机密码保存在 Vault 的 `lobbytally_database_password` 中。保持 `mpgs` 为私有 schema，浏览器通过 `/v1` API 访问数据。

此流程面向新项目。已用 SQL Editor 初始化的项目，应先按[部署指南](docs/VERCEL_SUPABASE.md)核对并登记迁移历史，再使用 `db push`，避免重跑基线。新部署不导入旧 SQLite 账号或游戏数据。

### 2. 配置 Vercel 项目和服务器变量

在 Vercel 导入仓库，**Root Directory 选择仓库根目录 `.`**，使用仓库声明的 Container Runtime 和 `Dockerfile.vercel`。若更改 Supabase 区域，同步调整 `vercel.json` 中的单区域配置。

在 Vercel 项目的 **Production Environment Variables** 中填写以下变量，完整示例见 [deploy/cloud.env.example](deploy/cloud.env.example)：

| 变量 | 值或来源 |
| --- | --- |
| `PORT` | `8080`。 |
| `MPGS_DATABASE_URL` | `postgresql://lobbytally_runtime.PROJECT_REF:PASSWORD@POOLER_HOST:5432/postgres?sslmode=require`；主机从 Supabase **Connect → Session pooler** 复制，密码取自 Vault，URL 特殊字符须编码。 |
| `MPGS_SUPABASE_URL` | `https://PROJECT_REF.supabase.co`。 |
| `MPGS_SUPABASE_SERVICE_ROLE_KEY` | Supabase 的服务端 service role key。 |
| `MPGS_CRON_SECRET` | 独立生成的随机值，至少 32 个非空白字节；下一步在 Vault 中使用同一值。 |
| `MPGS_TRUST_PROXY_HEADERS` / `MPGS_RATE_LIMIT_ENABLED` | 均设为 `true`。 |
| `MPGS_AI_PROVIDER` | `disabled`；启用 AI 时再填写示例中的 provider 配置。 |
| `MPGS_ADMIN_TOKEN` | 可选，使用管理 API 时配置独立令牌。 |
| `MPGS_STEAM_WEB_API_KEY` | 可选，启用 Steam 官方目录同步；未配置时仍可发现商店候选游戏。 |

这些密钥只用于服务端，不能加 `VITE_` 前缀或提交到仓库。当前驱动要求 **Session pooler 的 5432 端口**；6543 的 Transaction pooler 不适用。Preview 部署应使用独立 Supabase 项目和凭据。

可以通过 Vercel 的 Git 集成部署，也可以在根目录使用已验证的 CLI 版本关联同一项目并发布：

```powershell
npx --yes vercel@64.5.0 login
npx --yes vercel@64.5.0 link --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 deploy --dry --json --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 deploy --prod --skip-domain --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 inspect YOUR_DEPLOYMENT_ID --json --scope YOUR_VERCEL_TEAM
```

确认新部署 `readyState=READY` 且 `/health/ready` 返回应用 JSON 后，运行 `npx --yes vercel@64.5.0 promote YOUR_DEPLOYMENT_ID --scope YOUR_VERCEL_TEAM`。受保护部署的检查方式见[部署指南](docs/VERCEL_SUPABASE.md)。在 Deployment Protection 使用 Standard Protection，然后通过公开正式域名再次检查：

```powershell
$siteOrigin = 'https://YOUR_PROJECT.vercel.app'
Invoke-RestMethod "$siteOrigin/health/live"
Invoke-RestMethod "$siteOrigin/health/ready"
Invoke-RestMethod "$siteOrigin/v1/meta"
```

`/health/ready` 应返回 `status: ready`、`database: ok`。正式域名需要允许公开访问，Supabase Cron 才能请求它；Vercel 登录页不算健康检查通过。刚初始化的空目录也能通过就绪检查。

### 3. 开启采集并验证自动运行

在 Supabase Vault 中设置 `lobbytally_site_origin` 为正式 HTTPS origin（不含 API 路径），设置 `lobbytally_cron_secret` 为 Vercel 中相同的 `MPGS_CRON_SECRET`，然后执行：

```powershell
npx --yes supabase@2.121.0 db query --linked --file supabase/enable-schedules.sql
```

两个任务分别调用 `POST /internal/v1/maintenance` 和 `POST /internal/v1/avatar-cleanup`。游戏目录会随批次逐步填充；验收时同时检查 Cron 运行记录、实际 HTTP 返回和数据库游标是否推进。需要暂停时执行 [supabase/disable-schedules.sql](supabase/disable-schedules.sql)。

**后续更新继续使用同一 Supabase 和 Vercel 项目。** 有新增迁移时先检查迁移列表并用 `db push --linked --skip-vault` 应用待执行文件，再部署应用；普通应用更新直接重新部署，不重建数据库。完整验收、域名切换和密钥轮换见[中文部署指南](docs/VERCEL_SUPABASE.md) / [English deployment guide](docs/VERCEL_SUPABASE.en.md)。让代理协助操作时，先阅读 [AGENTS.md](AGENTS.md) 和 [代理部署指南](docs/AGENT_DEPLOYMENT.md)。

## 源码运行

适合修改代码或开发桌面客户端；线上部署使用上面的 Vercel + Supabase 流程。

前置环境：**Rust 1.97+、Node.js 22、pnpm 9.15.9、Git 和 PostgreSQL 17+**。按照 [云端部署指南](docs/VERCEL_SUPABASE.md) 配置数据库和服务器环境变量。

### 1. 获取代码

```powershell
git clone https://github.com/Lotulune/lobbytally.git
cd lobbytally
pnpm install --frozen-lockfile
```

### 2. 启动 API

将 `MPGS_DATABASE_URL`、Supabase Storage 和 Cron 变量导出到当前终端。新的本地 PostgreSQL 数据库先使用具备迁移权限的连接初始化：

```powershell
cargo run -p mpgs-dbtool --locked -- migrate
```

已初始化的数据库直接启动 API：

```powershell
cargo run -p mpgs-server --locked
```

API 默认监听 `http://127.0.0.1:17880`。本地 `dbtool migrate` 初始化应用 schema；Supabase 的 Storage、Vault 和 Cron 平台配置按上方部署流程完成。空目录可以正常启动；线上采集由 Supabase Cron 触发。

### 3. 启动 Web

保持 API 运行，另开一个终端，在仓库根目录执行：

```powershell
pnpm web:dev
```

打开 [http://localhost:5173](http://localhost:5173)。Vite 会将 API 请求代理到本地服务端。

### 验证改动

按改动范围选择检查；仅文档变更检查内容、差异和链接即可。需要运行数据库测试时，先将 `MPGS_TEST_DATABASE_URL` 设为独立、可丢弃的 PostgreSQL 测试数据库连接字符串。

```powershell
pnpm web:test
pnpm web:build
cargo fmt --all -- --check
cargo test --workspace --locked -- --test-threads=4
cargo clippy --workspace --all-targets --locked -- -D warnings
```

桌面开发需要额外的平台依赖，详见 [桌面 / Web 客户端指南](web/README.md)。

### CI 怎么运行

[CI 工作流](.github/workflows/ci.yml) 使用 `CI routing` 判断改动范围，`CI result` 汇总结果：

- 仅文档变更走轻量路径，跳过 Web、Rust、Linux 发布构建和容器构建。
- 代码变更运行 Web 测试与构建、Rust 格式 / 测试 / Clippy、Linux 构建和 Vercel 容器构建。
- 手动运行且 `full=false` 时执行常规检查；`full=true` 或 `v*` 标签额外触发完整桌面和多平台检查。

PR 按相对目标分支的完整改动判断，推送按本次提交范围判断。含代码的 PR 即使最新一次提交只改 README，仍会执行常规检查；无法确定范围时也执行常规检查。

需要配置分支保护时，推荐将稳定的 `CI result` 设为必需检查。GitHub CI 的运行结果在对应提交的 Actions 页面查看；本地验证与云端发布分别执行。

此分流仅控制 GitHub Actions；另行连接的 Vercel Git 自动部署遵循 Vercel 项目的构建触发规则。

## 技术与目录

前端使用 **React + TypeScript + Vite**，桌面外壳使用 **Tauri 2**；服务端使用 **Rust + Axum + Tokio**，以 **Supabase PostgreSQL** 保存目录、推荐数据与反馈。基础推荐由确定性规则生成，AI 是可配置的增强能力。

| 路径 | 职责 |
| --- | --- |
| `web/` | 浏览器与桌面共用的前端、主题、缓存和反馈交互。 |
| `apps/server/` | HTTP API、会话与账号、推荐接口、静态前端和鉴权后的维护入口。 |
| `apps/desktop/` | Tauri 桌面客户端。 |
| `apps/dbtool/` | PostgreSQL 迁移、采集与审计 CLI，以及隔离的本地 E2E 数据夹具。 |
| `crates/worker/` | 有时间预算、持久化游标与租约的采集和维护批次。 |
| `crates/` | 领域模型、推荐器、Steam 数据适配、PostgreSQL 存储与 AI。 |
| `supabase/` | PostgreSQL 迁移、私有存储及 Cron 配置。 |
| `Dockerfile.vercel` / `vercel.json` | 前端与 API 的容器构建、区域、路由和执行时长配置。 |
| `deploy/` | 当前云端环境变量示例及保留的历史自托管脚本。 |
| `docs/` | 产品、架构、开发与运维文档。 |

客户端通过 API 访问服务端；持久状态存放在 Supabase，Vercel 实例不依赖本地数据库文件或常驻后台进程。

## 继续了解

| 文档 | 内容 |
| --- | --- |
| [Vercel + Supabase 部署](docs/VERCEL_SUPABASE.md) | 当前部署、迁移、更新、密钥管理与线上验收。 |
| [代理部署指南](docs/AGENT_DEPLOYMENT.md) / [AGENTS.md](AGENTS.md) | 代理操作项目时的入口、约束与核验步骤。 |
| [HTTP API](docs/API.md) | 接口与数据契约。 |
| [系统架构](docs/ARCHITECTURE.md) | 组件职责与客户端 / 服务端边界。 |
| [推荐算法](docs/RECOMMENDATION.md) | 排序信号、推荐解释与校准。 |
| [AI 集成](docs/AI.md) | Provider、检索、配置与回退。 |
| [已知限制](docs/KNOWN_LIMITATIONS.md) | 当前的数据、产品与技术边界。 |
| [隐私说明](docs/PRIVACY.md) | 数据处理与隐私约定。 |
| [历史开发指南](docs/DEVELOPMENT.md) / [历史运维手册](docs/OPERATIONS.md) | 保留的 SQLite / VPS 阶段资料；当前部署以本 README 和云端部署指南为准。 |

欢迎通过 [Issues](https://github.com/Lotulune/lobbytally/issues) 反馈体验、错误数据或功能建议。报告问题时，请附上复现步骤、使用平台与相关截图；不要提交密钥、访问令牌或个人数据库。
