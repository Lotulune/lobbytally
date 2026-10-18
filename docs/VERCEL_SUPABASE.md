# Vercel + Supabase 部署

**简体中文** · [English](VERCEL_SUPABASE.en.md) · [Agent 部署指南](AGENT_DEPLOYMENT.md)

这条部署路径从空库开始，仅使用 Vercel 和 Supabase。Rust API 与现有 React 前端放在同一个 Vercel Container Runtime 服务中；PostgreSQL、私有头像存储、定时触发由 Supabase 提供。游戏数据由采集批次逐步填充，不需要常驻 worker 或 VPS。

“空库”适用于首次部署；更新已有项目时保留数据库、头像和任务进度。下文的占位符用于部署自己的项目；维护当前正式项目时，先核对 [生产状态](PRODUCTION_STATUS.md)。命令使用本次验证过的 Supabase CLI `2.121.0` 和 Vercel CLI `64.5.0`，升级版本时先检查对应命令的 `--help`。

## 1. 创建 Supabase 项目并初始化

在 [Supabase 控制台](https://supabase.com/dashboard) 创建项目，并为后续 Vercel 函数选择相近或相同的区域。应用使用专用数据库角色，不需要把项目的 `postgres` 管理密码交给 Vercel。

建议从首次初始化起就通过 CLI 管理迁移历史，在仓库根目录执行：

```text
npx --yes supabase@2.121.0 login
npx --yes supabase@2.121.0 init
npx --yes supabase@2.121.0 link --project-ref YOUR_PROJECT_REF
npx --yes supabase@2.121.0 migration list --linked
npx --yes supabase@2.121.0 db push --linked --skip-vault --dry-run
npx --yes supabase@2.121.0 db push --linked --skip-vault
```

已有登录、关联或 `supabase/config.toml` 时先核对目标，跳过相应初始化步骤。`--skip-vault` 跳过 CLI 从 `config.toml` 同步 Vault 配置；迁移 SQL 中创建 Vault secret 的语句仍会执行。

CLI 按文件名顺序应用缺少的迁移。当前基线包含：

1. `supabase/migrations/20261018174837_lobbytally_initial.sql`
2. `supabase/migrations/20261018174851_lobbytally_platform.sql`
3. `supabase/migrations/20261018175423_lobbytally_postgres_hardening.sql`
4. `supabase/migrations/20261018182444_lobbytally_runtime_access.sql`

第一份文件创建私有 `mpgs` schema、表、索引、触发器和运行默认值。第二份文件创建私有 `avatars` bucket 和定时请求函数；第三份调整扩展位置并补齐外键索引；第四份创建仅供后台使用的 `lobbytally_runtime` 登录角色和 RLS 策略，并将随机密码加密保存在 Vault 的 `lobbytally_database_password` 中。文件适用于新项目；不要反复执行第一份基线。

也可以在 SQL Editor 依次执行上述文件，但它不会写入 CLI 迁移记录。仅在确认四份 SQL 都已成功执行后，才可登记已有基线，避免 `db push` 重跑初始化：

```text
npx --yes supabase@2.121.0 migration repair 20261018174837 20261018174851 20261018175423 20261018182444 --status applied
```

`migration repair` 只登记历史，不执行 SQL。也可以从第一次初始化起就使用关联项目的 CLI `db push`，此时无需手动执行或修复这些迁移。[官方迁移记录说明](https://supabase.com/docs/reference/cli/supabase-migration-repair)

保持 `mpgs` 不在 Data API 的 exposed schemas 中。浏览器仍通过现有 `/v1` API 访问数据；账号登录沿用应用自己的账号体系，无需启用 Supabase Auth。头像 bucket 不添加公开读取策略，公开头像由 API 审核后返回。

从项目的 **Connect → Session pooler** 复制端口 `5432` 的主机名，将用户名设为 `lobbytally_runtime.PROJECT_REF`，密码使用 Vault 中的 `lobbytally_database_password`，并加入 `sslmode=require`。不要使用 `6543` transaction pooler：当前 Rust 驱动使用会话设置和预编译语句。密码中的 URL 特殊字符必须进行百分号编码。使用具备管理权限的 CLI 或 SQL Editor 读取密码后，直接写入被 Git 忽略的本地 `deploy/cloud.env` 或 Vercel 的私密环境变量，不要写进 SQL 文件或版本库。

数据库客户端会为官方 Supabase 数据库和 pooler 主机追加仓库内的官方根证书 `crates/storage/certs/supabase-prod-ca-2021.crt`，同时保留系统根、证书链验证和主机名验证。证书来自 [Supabase 官方下载](https://supabase-downloads.s3-ap-southeast-1.amazonaws.com/prod/ssl/prod-ca-2021.crt)，SHA-256 为 `700723581420dd1ac98fd7e9ac529f0ef210eadcaf87fc868a3ad7d114c2f3b7`。

## 2. 创建 Vercel 项目

在 [Vercel 控制台](https://vercel.com/new) 导入此仓库，项目根目录选仓库根目录。仓库中的 `vercel.json` 声明容器服务和东京区域 `hnd1`，`Dockerfile.vercel` 构建前端和 Rust API。不要将 Root Directory 设为 `web`。若 Supabase 使用其他区域，应同步修改 Vercel 的单区域配置。

Container Runtime 当前为 Beta。若账号尚不能使用该运行时，应先确认账号可用性再切换域名。

在 Deployment Protection 中使用 Standard Protection，确保正式域名允许公开访问、预览部署仍需认证。仅看到 HTTP `200` 不够：登录页重定向最终也可能返回 `200`；健康检查必须返回应用的 JSON，且 `database` 为 `ok`。

按照 `deploy/cloud.env.example` 填入服务器环境变量，至少配置：

| 变量 | 来源 |
| --- | --- |
| `PORT` | `8080`，让 Vercel 路由端口与容器监听端口一致 |
| `MPGS_DATABASE_URL` | Supabase 的 Session pooler URL，端口 5432，启用 TLS |
| `MPGS_SUPABASE_URL` | 项目 HTTPS URL |
| `MPGS_SUPABASE_SERVICE_ROLE_KEY` | 项目的服务端 service role key |
| `MPGS_CRON_SECRET` | 独立随机值，至少 32 个非空白字节 |
| `MPGS_TRUST_PROXY_HEADERS` | `true` |
| `MPGS_AI_PROVIDER` | 未配置 AI 时设为 `disabled` |

可另外设置 `MPGS_ADMIN_TOKEN` 以使用管理 API。`MPGS_STEAM_WEB_API_KEY` 可选；不填时仍能采集 Steam Store 候选，填入后启用官方目录同步。

可以把模板复制到被 Git 忽略的 `deploy/cloud.env` 作为本地私密配置，再通过 Vercel 控制台或 `env add` 设置 Production 变量。该文件不会自动上传成环境变量。

```text
npx --yes vercel@64.5.0 login
npx --yes vercel@64.5.0 link --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 env add MPGS_DATABASE_URL production --sensitive --scope YOUR_VERCEL_TEAM
```

其余变量按模板逐项配置。`env add` 会交互读取值；自动化通过标准输入传入私密值，避免把它们放入命令行参数、日志或聊天。

这些变量均属于服务端，不能改为 `VITE_` 前缀。建议先只配置 Production 环境；需要 Preview 时使用独立 Supabase 项目和独立凭据，以免预览实例修改正式数据。

`vercel.json` 已为容器函数配置 120 秒最大执行时长。应用 HTTP 截止时间为 60 秒，采集批次默认 45 秒，头像清理有独立 45 秒截止时间。部署后检查 `/health/live`、`/health/ready`、`/v1/meta`；初始化后的空目录可以通过就绪检查。

CLI 发布流程如下；远端构建不要求本机安装 Docker：

```text
npx --yes vercel@64.5.0 deploy --dry --json --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 deploy --prod --skip-domain --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 inspect YOUR_DEPLOYMENT_ID --json --scope YOUR_VERCEL_TEAM
```

检查 dry-run 清单包含 Rust / Web 构建输入、Supabase SQL 和官方 CA，排除了 `.tmp/`、环境文件、数据库、`node_modules/` 和 `target/`。dry-run 只验证上传输入，不能替代真实构建。`maxDuration` 配置位于 `services.app.functions` 下。

确认部署的 `readyState` 为 `READY`，再检查新部署的 `/health/ready`。受保护的部署可使用已登录 CLI 的 `vercel curl /health/ready --deployment YOUR_DEPLOYMENT_ID`。必须取得包含 `status: "ready"`、`database: "ok"` 的应用 JSON，之后执行：

```text
npx --yes vercel@64.5.0 promote YOUR_DEPLOYMENT_ID --scope YOUR_VERCEL_TEAM
```

提升后再次直接访问正式地址验证，无需平台绕过令牌。自定义域名先在 Vercel 项目中添加，按项目返回的值完成 TXT 所有权验证和 CNAME 配置，Cloudflare 使用 DNS only。证书及正式域名 API 正常后再配置下面的 Vault origin；不要复制其他项目的域名验证 token。

## 3. 开启定时采集

在 Supabase 的 Vault 中创建两个命名 secret：

| 名称 | 值 |
| --- | --- |
| `lobbytally_site_origin` | 公开的正式站点 HTTPS origin，例如 `https://your-project.vercel.app`，不含 API 路径 |
| `lobbytally_cron_secret` | 与 Vercel 的 `MPGS_CRON_SECRET` 完全相同 |

使用正式部署地址；该地址必须允许 Supabase 请求访问内部端点，鉴权由应用的独立 Cron secret 完成。

先用该 secret 调用 `POST /internal/v1/maintenance`，JSON 请求体为 `{}`，鉴权头为 `Authorization: Bearer <secret>`，核对实际批次结果。然后在 SQL Editor 执行 `supabase/enable-schedules.sql`，或使用 CLI：

```text
npx --yes supabase@2.121.0 db query --linked --file supabase/enable-schedules.sql --output json
```

- 每分钟调用 `POST /internal/v1/maintenance`，执行有限批次采集及维护。
- 每 5 分钟调用 `POST /internal/v1/avatar-cleanup`，清理已经退休且不再被账号引用的头像对象。

Cron SQL 只保存 Vault secret 的名称。任务游标、冷却时间和租约保存在 PostgreSQL；重复触发不会同时启动两批采集，实例中断后任务会在租约到期后恢复。

可以用以下查询检查调度和实际 HTTP 结果。Cron 成功入队只代表已发起请求，还要检查 HTTP 状态。

```sql
SELECT jobname, schedule, active FROM cron.job
WHERE jobname LIKE 'lobbytally-%';

SELECT jobid, status, return_message, start_time, end_time
FROM cron.job_run_details ORDER BY start_time DESC LIMIT 20;

SELECT id, status_code, timed_out, error_msg, created
FROM net._http_response ORDER BY created DESC LIMIT 20;

SELECT cursor_key, updated_at_ms FROM mpgs.source_cursors
ORDER BY updated_at_ms DESC LIMIT 10;
```

停止调度时执行 `supabase/disable-schedules.sql`。更换域名时更新 Vault 的 origin；轮换 Cron secret 时同时更新 Vault、Vercel 环境变量并重新部署。

## 4. 上线验收

从空库开始，先确认健康检查成功，再等待定时任务填充目录。随后检查注册登录、偏好保存、游戏搜索及详情、想玩投票、头像上传/删除、推荐和可选 AI。检查数据库中的游标随多个批次前进，并确认无浏览访问时 Cron 仍能继续更新数据。

原 VPS 只有在上述功能和自动采集在云端通过后才停止。当前部署不会读取旧 SQLite 文件，也不会自动导入旧账号或数据。

## 5. 更新已有项目

核对本地关联的 Vercel / Supabase 项目及迁移历史，仅应用新增迁移，再按“构建 → READY → 健康检查 → promote”的顺序更新代码。环境变量变化需要新部署才能生效。数据库变更应兼容切换期间仍在运行的旧版本；回滚容器不会撤销数据库迁移。头像删除接口成功后，还需核对 Storage 对象或清理任务是否实际完成。

## 本地开发与验证

准备独立 PostgreSQL 数据库（建议 PostgreSQL 17+，包含 `pg_trgm`）。只把测试连接指向可丢弃的数据库；测试为每个用例创建并清理独立 schema。

`deploy/cloud.env` 使用逐行、不带外层引号的 `KEY=value` 格式；服务端不会自动加载文件。本地运行时显式加载到进程环境，例如 PowerShell：

```powershell
$ErrorActionPreference = 'Stop'
foreach ($line in Get-Content -LiteralPath 'deploy/cloud.env' -Encoding UTF8) {
    if ($line -match '^([A-Z][A-Z0-9_]*)=(.*)$') {
        [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
    }
}
```

本地初始化使用有建表权限的开发数据库角色，并将 URL 换成独立开发库。本地头像功能使用开发用 Supabase Storage。桌面 E2E 还需要能够创建临时数据库的本地测试角色。

```powershell
$env:MPGS_TEST_DATABASE_URL = 'postgresql://postgres:password@127.0.0.1:5432/lobbytally_test?sslmode=disable'
cargo test --workspace --locked -- --test-threads=4
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
pnpm web:test
pnpm web:build
```

本地 API 使用 `MPGS_DATABASE_URL` 和同一套服务器环境变量。先执行 `cargo run -p mpgs-dbtool -- migrate`，然后启动 `cargo run -p mpgs-server` 与 `pnpm web:dev`。生产服务启动时只检查数据库，不自动执行迁移或填充演示游戏。

纯文档修改只需审阅差异和检查链接。GitHub CI 会跳过对应的 Web、Rust、Linux 和容器构建；代码、配置、SQL 或未知类型修改仍执行普通检查。手动 `full=true` 和 `v*` 标签运行完整桌面 / 多平台检查，`CI result` 始终汇总结果。该策略仅控制 GitHub Actions；另行连接的 Vercel Git 自动部署遵循 Vercel 项目的构建触发规则。

平台依据：[Vercel Rust 容器部署](https://vercel.com/kb/guide/deploy-rust-on-vercel-with-docker)、[Supabase 数据库连接](https://supabase.com/docs/guides/database/connecting-to-postgres)、[Supabase 定时 HTTP 调用](https://supabase.com/docs/guides/functions/schedule-functions)。
