# 本地开发指南

## 1. 当前基线

仓库当前 **M3–M5 已验收关闭**；M6 发布加固工程基线见 [`M6_ACCEPTANCE.md`](M6_ACCEPTANCE.md)。M5 审查修复记录见 [`M5_ACCEPTANCE_RUN.md`](M5_ACCEPTANCE_RUN.md)。发布前数据富化、合规签字与真实签名仍须并行/人工完成。

- `mpgs-domain`：分区、偏好、反馈类型与推荐信号。
- `mpgs-recommender`：评分、个性化、硬过滤、MMR、解释与 `rank_feed`。
- `mpgs-steam-source`：Steam 源规范化适配器、多人搜索页 HTML 解析器与黄金集。
- `mpgs-storage`：SQLite 迁移（含用户/偏好/反馈/可用性/游玩意愿票）、单写与并发只读连接、Repository、种子目录、查询与备份。
- `mpgs-server`：公开 API（会话/偏好/四分区/日历/搜索/详情/证据/反馈/游玩意愿投票/自然语言确定性 fallback）、生成式 OpenAPI、限流、`x-request-id`、ETag、CORS 白名单；管理/内部 jobs。
- `mpgs-dbtool`：migrate / Steam 候选采集 / integrity / m3-audit / backup / restore。
- 桌面：`web/`（Vite + React + TS，多主题/离线缓存/自然语言 UI）+ `apps/desktop/src-tauri/`（Tauri 2）+ `e2e-tests/`（Windows/Linux `tauri-driver`）。
- M5 起步：`mpgs-ai` Provider/Gateway 已接入；默认 `MPGS_AI_PROVIDER=disabled`。可选 `openai_compat` + `MPGS_AI_API_KEY`（及 `MPGS_AI_BASE_URL`/`MPGS_AI_MODEL`/`MPGS_AI_TIMEOUT_SECS`）。关闭或失败时自然语言仍返回确定性结果与 `ai_status=fallback`。
- 检索索引：`mpgs-dbtool sync-retrieval <db> [limit] [after_app_id]` 增量同步 `game_documents`/`game_fts`/`game_embeddings`（hash-embed）。自然语言推荐会在文档为空时自动同步一次（上限 2000）。
- 离线特征：`mpgs-dbtool extract-offline-features <db> [limit] [after_app_id]`。
- Embedding：`MPGS_AI_EMBED_PROVIDER=hash|openai_compat|disabled`；openai_compat 时用 `MPGS_AI_EMBED_MODEL`（默认 `text-embedding-3-small`）与 `MPGS_AI_EMBED_DIMENSIONS`。
- Hash Embedding 当前版本为 `hash-embed-v2`；已有数据库升级后运行一次 `sync-retrieval` 或 `embed-documents` 重建派生向量。
- 批处理：`mpgs-dbtool embed-documents <db> [limit] [batch]`；离线验收：`.\scripts\m5_acceptance.ps1`（见 [M5_ACCEPTANCE.md](M5_ACCEPTANCE.md)）。

M4 关闭证据：本机验收与 E2E 见 [`M4_ACCEPTANCE.md`](M4_ACCEPTANCE.md)；跨平台 CI 全绿见 [`M4_CI_RUN.md`](M4_CI_RUN.md)（[run 29497583493](https://github.com/Lotulune/mpgs/actions/runs/29497583493)，commit `5e0274b`）。
### Git

本机已使用 Git for Windows。新终端若找不到 `git`，将 `C:\Program Files\Git\cmd` 加入 PATH，或在当前会话执行：

```powershell
$env:Path = "C:\Program Files\Git\cmd;" + $env:Path
```

仓库本地配置（不写全局）：`user.name` / `user.email` 仅限本仓库；`core.autocrlf=true`。

## 2. 前置环境

- Rust stable，最低 `1.97`，包含 `rustfmt` 与 `clippy`。
- Git，用于正常版本管理；当前代码不依赖 Git 才能编译。
- Node.js LTS 与 pnpm，仅在建立 Tauri/React 客户端后需要。
- Windows 开发 Tauri 时需要 Microsoft C++ Build Tools 与 WebView2。
- Linux 开发 Tauri 时需要对应发行版的 WebKitGTK 4.1 开发包。
- macOS 打包需要 macOS/Xcode 环境，不能把正式签名产物建立在非 macOS 主机上。

## 3. 常用命令

```powershell
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p mpgs-storage --locked
cargo run -p mpgs-server
```

`mpgs-steam-source` / `mpgs-storage` 默认测试只使用夹具与临时库，不调用实时 Steam，也不需要 Web API Key。

带本地 SQLite 启动服务端：

```powershell
New-Item -ItemType Directory -Force data | Out-Null
$env:MPGS_DATABASE_PATH = '.\data\mpgs.db'
$env:MPGS_ADMIN_TOKEN = 'dev-only-token'
$env:MPGS_SEED_DEMO = 'true' # 仅本地演示；省略时持久化空库保持为空
cargo run -p mpgs-server
```

运行后验证：

```powershell
Invoke-RestMethod 'http://127.0.0.1:8080/health/live'
Invoke-RestMethod 'http://127.0.0.1:8080/health/ready'
Invoke-RestMethod 'http://127.0.0.1:8080/v1/meta'
Invoke-RestMethod 'http://127.0.0.1:8080/openapi.json'
```

数据库工具：

```powershell
cargo run -p mpgs-dbtool -- migrate .\data\mpgs.db
cargo run -p mpgs-dbtool -- integrity .\data\mpgs.db
cargo run -p mpgs-dbtool --locked -- collect-steam-candidates .\data\m3-real.db 2000
cargo run -p mpgs-dbtool --locked -- import-golden-profiles .\data\m3-real.db
cargo run -p mpgs-dbtool --locked -- enrich-steam-candidates .\data\m3-real.db 100
cargo run -p mpgs-dbtool -- m3-audit .\data\m3-real.db
cargo run -p mpgs-dbtool -- backup .\data\mpgs.db .\backups\mpgs.db
cargo run -p mpgs-dbtool -- restore .\backups\mpgs.db .\data-restored\mpgs.db
# 或
.\scripts\backup_db.ps1 -DbPath .\data\mpgs.db -OutPath .\backups\mpgs.db
.\scripts\restore_db.ps1 -From .\backups\mpgs.db -To .\data-restored\mpgs.db
```

`collect-steam-candidates` 使用 Steam 商店搜索的 `category2=1` 多人分类和 `Reviews_DESC` 排序。该接口没有稳定公开契约，因此实现位于独立易变适配器中：响应限制为 4 MiB，HTML 通过 DOM 解析器读取，失败最多退避重试 3 次，成功页间隔至少 1.1 秒；每页写入 `source_documents`、`feature_evidence`、`source_runs` 和 `source_cursors`。命令可安全续传，但它只建立低置信候选证据，不会推断合作、自建服或私人房间能力。

`import-golden-profiles` 将带版本、内容哈希和原始文档的嵌入式黄金集幂等写入 `multiplayer_profiles` 与 `feature_evidence`（不覆盖人工 override），用于提升 `recommendation_ready_profiles` / `trusted_familiar_profiles`。`enrich-steam-candidates` 对多人候选按轮转游标抓取 `appdetails`、评价摘要与 CCU：平台/语言缺失时补抓，评价和价格每 24 小时到期，CCU 每 6 小时到期；默认每次 100 个 App，请求间隔约 1.1 秒。商店区域默认 `CN/schinese`，可用 `MPGS_STEAM_COUNTRY`、`MPGS_STEAM_LANGUAGE` 覆盖。深度熟人联机画像仍依赖黄金集与人工校正，不由商店分类自动推断。

2026-07-14 的本地门禁结果：`normalized_multiplayer_candidates=2071`、`category_evidence_candidates=2071`、`recommendation_ready_profiles=0`。后两个指标必须分开阅读，不能把分类覆盖当成推荐质量。

2026-07-16 本地审计结果（`data/m3-real.db`）：候选 2091，`recommendation_ready_profiles=50`、`trusted_familiar_profiles=14`、`with_platforms=2091`、`with_languages=2090`、`with_reviews=2091`、`with_ccu=2091`、`with_price=2081`。`with_price` 只统计有实际金额的快照；免费游戏记 0 价，商店未返回币种或金额时不再伪造 USD 价格。数据库中历史 `US/USD` 快照不代表中国区价格覆盖完成，需继续按默认 `CN/schinese` 运行富化刷新。`with_typical_session=0`，典型局时长仍需人工校准。

服务默认绑定 `127.0.0.1:8080`。仅在本地端口冲突时临时设置进程变量：

```powershell
$env:MPGS_BIND_ADDR = '127.0.0.1:8081'
cargo run -p mpgs-server
```

不要把本地地址、Key 或个人路径提交为默认配置。

## 3.1 桌面客户端（M4）

前端在 `web/`，Tauri 壳在 `apps/desktop/src-tauri/`，详见 [web/README.md](../web/README.md)。

```powershell
pnpm install
# 另开终端启动带演示数据的服务端：
$env:MPGS_SEED_DEMO = 'true'; cargo run -p mpgs-server
# 浏览器开发（Vite 代理 /v1 到 127.0.0.1:8080）：
pnpm --filter mpgs-web dev            # http://localhost:5173
# 校验：
pnpm --filter mpgs-web typecheck
pnpm --filter mpgs-web test
pnpm --filter mpgs-web build
# M4 API 级验收（自动起临时演示服务端 + web test/build + desktop cargo check）：
.\scripts\m4_acceptance.ps1
# Windows/Linux 原生桌面 E2E（需先安装 tauri-driver 与平台 WebDriver）：
pnpm desktop:e2e:build
pnpm desktop:e2e
# Windows 桌面安装包冒烟（未签名；NSIS 会按需下载工具链）：
pnpm exec tauri build --config apps/desktop/src-tauri/tauri.conf.json --ci --no-sign -b nsis
pnpm exec tauri build --config apps/desktop/src-tauri/tauri.conf.json --ci --no-sign -b msi
```

验收说明与结果：[M4_ACCEPTANCE.md](M4_ACCEPTANCE.md)。PRD 7.2 已提供确定性自然语言意图解析和推荐流程；未配置外部 AI Provider 时响应明确标记 `ai_status=fallback`，不谎报 AI 可用。

Tauri 壳是独立 Cargo workspace，不进入根 workspace，因此 `cargo test --workspace` 与 CI 不需要 WebView 工具链。仓库 devDependency 含 `@tauri-apps/cli`。Windows 打包依赖 MSVC + WebView2；NSIS/WiX 由 Tauri 首次构建时拉取。Linux 需 WebKitGTK 4.1；macOS 需 Xcode 与原生 runner。桌面客户端默认把状态写入应用私有数据目录；自动化测试可设置 `MPGS_CLIENT_DATA_DIR` 使用隔离目录。

## 4. Workspace 依赖方向

允许：

```text
server -> storage/steam-source/recommender/domain
dbtool -> storage/steam-source（仅运维采集命令）
recommender -> domain
storage -> domain + steam-source（仅 proposal 类型）
steam-source -> domain
desktop Rust -> api-contract/domain（必要时）
```

禁止：

```text
domain -> Axum/SQLite/AI SDK/Tauri
recommender -> HTTP/数据库/具体 AI Provider
source adapter -> UI
客户端 -> storage 或服务端密钥模块
```

领域逻辑应能使用纯结构体和夹具测试，不需要网络和数据库。

## 5. Rust 代码约定

- 新 crate 默认 `#![forbid(unsafe_code)]`；确需 unsafe 的底层依赖封装必须单独评审。
- 公共边界使用强类型、枚举和范围校验，不用自由字符串穿透领域层。
- 外部错误在适配器边界转换为稳定业务错误，不根据供应商错误文本驱动逻辑。
- 网络请求不得持有 SQLite 写事务。
- 时间由可注入 Clock 提供，测试不依赖真实当前时间。
- 随机探索由可注入种子提供，推荐测试必须可重现。
- 日志使用结构化字段，禁止日志拼接 Key、令牌、Cookie、完整 Prompt 或私人文本。
- `unwrap/expect` 只用于测试或编译期不变量；服务请求路径返回可分类错误。

## 6. 推荐器开发

- 所有输入在边界归一化为 `[0,1]`；NaN、无穷和越界值必须显式处理。
- 改变公式、阈值或缺失值策略时更新算法配置版本。
- 新规则至少添加一个正例、一个反例和一个边界测试。
- 黄金游戏的真实标签不应直接硬编码进公式；测试使用它们代表的特征信号。
- AI 分数不得在推荐器之外私自混合，统一经过受限融合函数。

## 7. 外部数据适配器

每个来源独立 crate/module，并实现统一阶段：

```text
request -> raw response validation -> source DTO -> normalized proposal
```

要求：

- 明确 User-Agent、限流、超时、重试和最大响应大小。
- DTO 与领域模型分离，外部字段变化不直接污染数据库 Schema。
- 保存脱敏夹具用于测试；默认测试不调用实时 Steam。
- 解析失败不得返回“成功空数据”。
- 适配器版本进入 `source_runs` 和原始文档元数据。
- 新增来源前更新 [数据来源表](DATA_STORAGE.md#2-数据源) 与 [外部资料](SOURCES.md)。

## 8. SQLite 开发

- 迁移放在 `migrations/NNNN_description.sql`，发布后不得修改已有迁移。
- 集成测试为每个用例创建独立临时数据库。
- 每次获取连接后验证必要 PRAGMA，不假设连接池自动继承。
- 生产文件只能由同机服务访问；测试也不要使用网络共享目录。
- 写事务保持短小，批量写使用有界批次。
- 文件数据库的查询使用独立只读连接；写入使用单写句柄。API handler 必须通过阻塞任务执行同步 SQLite 调用，不能占用 Tokio 工作线程。
- FTS、Embedding 和推荐快照必须能从权威表重建。

新增迁移的最低测试：

1. 空库升级。
2. 上一版本含数据数据库升级。
3. 外键、唯一键和 CHECK 约束。
4. 重复执行迁移入口不会静默破坏数据。
5. 备份恢复后可继续迁移。

## 9. API 开发

- 先更新 [API 契约](API.md)，再实现 DTO 与 handler。
- handler 只做协议、鉴权和输入校验；业务逻辑进入 service/domain。
- 列表端点使用不透明游标，不使用易漂移的公开 offset 分页。
- 写端点考虑幂等、乐观版本和审计。
- AI 回退属于成功响应的明确状态，不用 500 表示正常降级。
- 新端点必须有成功、输入错误、权限错误和依赖故障测试。

## 10. AI 开发

- 测试默认使用 Fake 或 Disabled Provider，不消耗真实额度。单元：`cargo test -p mpgs-ai`；集成：`cargo test -p mpgs-server natural_language`。
- 模型输出先经过 JSON Schema 和语义校验，再进入推荐器。
- 不提供通用 SQL、任意 URL 或文件工具。
- 所有模型可见外部文本均标记为数据，并进行大小/字符清洗。
- 真实 Provider 集成测试通过显式开关运行，不能成为普通测试前置条件。
- 不在仓库、命令历史示例或测试夹具中放真实 Key。

## 11. 客户端开发

- Tauri capabilities 使用最小权限，不开放任意 shell 或文件系统访问。
- UI 必须覆盖加载、空、错误、过期、离线和 AI 回退状态。
- 缓存响应与用户待同步反馈分开，清缓存不能丢失未提交反馈。
- 外部描述按纯文本/清洗内容渲染；只允许 HTTPS 和受控 Steam 链接。
- 桌面主工作流优先支持键盘，文本在所有目标窗口尺寸下不得溢出。

## 12. 配置和密钥

计划中的服务端密钥名称：

```text
MPGS_STEAM_WEB_API_KEY
MPGS_AI_API_KEY
MPGS_AI_API_BASE_URL
MPGS_AI_MODEL
MPGS_EMBEDDING_MODEL
```

这些 AI/Steam 变量当前尚未被代码读取。实现 Provider 时提供 `.env.example` 但不自动加载生产 `.env`，并确保日志只显示“已配置/未配置”。供应商 URL 是否允许自定义需要单独安全评审。

M3 已读取的非敏感限流变量：`MPGS_RATE_LIMIT_ENABLED`、`MPGS_RATE_LIMIT_READ_PER_MINUTE`、`MPGS_RATE_LIMIT_SEARCH_PER_MINUTE`、`MPGS_RATE_LIMIT_SESSION_PER_MINUTE`、`MPGS_RATE_LIMIT_FEEDBACK_PER_MINUTE`、`MPGS_RATE_LIMIT_GLOBAL_PER_MINUTE`、`MPGS_TRUST_PROXY_HEADERS`。代理部署只有在入口覆盖并清洗转发头时才能开启最后一项。

M4 新增的非敏感变量：服务端 `MPGS_CORS_ENABLED`、`MPGS_CORS_ALLOWED_ORIGINS`（逗号分隔精确源，默认覆盖 Tauri webview 源，见 [API 契约](API.md#171-corsm4)）；前端 `VITE_MPGS_API_BASE`（打包构建仅接受桌面 CSP 已允许的 `http://127.0.0.1:8080` 或 `http://localhost:8080`，开发留空走 Vite 代理，见 [web/.env.example](../web/.env.example)）。这些均非敏感，不含任何 Key。

## 13. 提交前检查

```powershell
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

同时确认：

- 文档和 OpenAPI 与行为一致。
- 没有真实 Key、令牌、数据库、日志或原始个人数据进入变更。
- 没有绕过证据、候选白名单或 SQLite 单主边界。
- 新外部事实附官方来源与核验日期。

GitHub Actions 构建 CI 已获负责人确认并创建于 `.github/workflows/ci.yml`：质量门禁运行在 Linux x64，发布构建使用 Windows/Linux 的 x64/ARM64 原生 runner，构建产物保留 14 天。发布、签名和部署仍需单独确认。

## 14. M6 发布加固

```powershell
# 离线验收（干净工作树才 PASS）
.\scripts\m6_acceptance.ps1
# 可选：再打带 PROVENANCE 的 release 服务包
.\scripts\m6_acceptance.ps1 -Package

# 单独打服务端包（注入 git SHA）
$env:MPGS_BUILD_GIT_SHA = (git rev-parse HEAD)
.\scripts\package_server.ps1

# 刷新第三方许可表
.\scripts\generate_third_party_licenses.ps1
```

运维与合规文档：`docs/OPERATIONS.md`、`ROLLBACK.md`、`KNOWN_LIMITATIONS.md`、`PRIVACY.md`、`SIGNING_AND_UPDATES.md`、`STEAM_BRAND_REVIEW.md`。Linux systemd 与 Windows WinSW 模板在 `packaging/`。
