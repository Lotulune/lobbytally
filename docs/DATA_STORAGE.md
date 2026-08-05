# 数据与存储规格

## 1. 数据原则

- SQLite 保存权威业务状态；FTS、向量和推荐快照都是可重建派生数据。
- 每个外部事实都保留来源、抓取时间、内容哈希和可信度。
- 当前值与历史快照分离，避免每次查询扫描历史表。
- 原始外部文档按必要性和保留期保存，不无限积累原始评论文本。
- 不把“缺失”写成 `false`，不把 AI 推断写成官方事实。
- 客户端缓存数据库与服务端权威数据库完全分离。

## 2. 数据源

| 数据 | 首选来源 | 说明 |
| --- | --- | --- |
| App 目录 | Steam `IStoreService/GetAppList` | 支持 AppID 分页、`last_modified` 和增量过滤，需要 Web API Key |
| 多人候选发现 | 经批准的 Steam 商店搜索适配器 | `category2=1` 仅作为低置信候选证据；接口/HTML 易变，不证明合作、自建服或私人房间能力 |
| 当前玩家数 | Steam `ISteamUserStats/GetNumberOfCurrentPlayers` | 单 AppID 当前在线，不能统计离线 Steam 玩家 |
| 评论摘要 | Steam Store Reviews API | 获取总正/负评价、评分描述和分页评论 |
| Demo 规则 | Steamworks Demo 文档与商店关系数据 | Demo 使用独立 AppID，需要关联本体 |
| 发售状态/日期 | 经批准的 Steam 商店适配器 | 没有一个公开 Web API 完整覆盖日历，必须单独做可行性与合规验证 |
| 多人能力 | Steam 明确字段、开发者说明、人工校正 | 标签只能作为证据，不能独立证明自建服或主导体验 |
| 价格/平台/语言 | 经批准的商店适配器 | 价格按地区和币种保存快照 |
| AI 摘要/特征 | MPGS AI 离线任务 | 派生数据，必须引用输入文档并保存模型版本 |

禁止以抓取 SteamDB 等第三方站点作为 MVP 基础数据源。使用第三方数据前必须取得许可并建立独立适配器，不得把其页面结构写进核心领域逻辑。

## 3. 采集策略

### 3.1 调度建议

| 任务 | 重点候选 | 长尾候选 |
| --- | --- | --- |
| App 目录增量 | 每日 | 同一全局任务 |
| 发售日/状态/Demo | 每 6 小时 | 每日 |
| 评论摘要 | 每 6 小时 | 每日或每 3 日 |
| CCU | 每 30 分钟 | 每 6～24 小时 |
| 价格 | 每 6 小时 | 每日 |
| 服务状态人工复核 | 事件触发 | 每月抽样 |
| AI 文档/Embedding | 内容哈希变化 | 进入重点候选时 |

重点候选包括当前推荐流、即将发售、用户近期检索和人工关注的游戏。

### 3.2 调用预算

Steam Web API 条款当前写明每日最多 `100,000` 次调用。调度器必须把每类请求放入共享令牌桶，并保留安全余量。

示例预算，不是固定配置：

```text
500 个重点 App，每 30 分钟 CCU       24,000/day
3,000 个长尾 App，每 6 小时 CCU      12,000/day
目录、详情、失败重试和保留余量        < 40,000/day
```

商店端点可能有不同或未公开的限流规则，必须使用独立限流器、清晰 User-Agent、缓存、指数退避和低并发。不能把 Web API 的 100,000 次预算理解为商店页面抓取授权。

M3 门禁采集的运行约束：每页最多 100 行、响应最多 4 MiB、成功页间隔至少 1.1 秒、最多 3 次指数退避重试；页级游标在成功写入后持久化。生产调度上线前仍需复核 Steam 条款与实际限流，不能把一次可行性运行视为长期抓取许可。

### 3.3 任务状态

每个采集任务使用：

```text
source + task_type + entity_key + due_at + priority
```

领取任务时写入短租约；提交必须携带 `job_id` 和幂等键。网络请求在数据库事务外执行，写入规范化结果时使用短事务。

## 4. 规范化流程

```mermaid
flowchart LR
    Fetch["获取外部响应"] --> Validate["状态/类型/大小校验"]
    Validate --> Hash["内容哈希与去重"]
    Hash --> Parse["来源专用解析"]
    Parse --> Normalize["规范化字段与枚举"]
    Normalize --> Resolve["来源优先级/人工覆盖"]
    Resolve --> Snapshot["写入历史快照"]
    Resolve --> Current["更新当前有效值"]
    Current --> Derived["FTS、向量、推荐特征失效"]
```

解析器失败时保留现有当前值并记录结构变化，不把空解析结果覆盖到数据库。

## 5. 标识和通用类型

- Steam AppID：SQLite `INTEGER`，Rust `u32`，API 返回数字；写入时校验 `0..=4294967295`。
- 内部实体 ID：UUIDv7 或单调整数，不能与 AppID 混用。
- 时间：数据库统一使用 UTC Unix 毫秒，字段后缀 `_at_ms`。
- 日期：只有日粒度的发售日期使用 ISO `YYYY-MM-DD` 文本，并保存精度枚举。
- 价格：整数最小货币单位，例如人民币分；同时保存 ISO 4217 币种与商店地区。
- 布尔值：SQLite `INTEGER CHECK(value IN (0,1))`；未知值使用 `NULL` 或显式枚举。
- 比例/分数：`REAL CHECK(value BETWEEN 0 AND 1)`。
- 枚举：MVP 使用受约束 `TEXT`，由应用和 CHECK 共同校验。

## 6. 逻辑数据模型

```mermaid
erDiagram
    apps ||--o{ app_localizations : has
    apps ||--o{ app_relations : source
    apps ||--o{ app_relations : target
    apps ||--o| multiplayer_profiles : has
    apps ||--o| app_availability : available_on
    apps ||--o{ feature_evidence : supports
    apps ||--o{ review_snapshots : receives
    apps ||--o{ player_snapshots : has
    apps ||--o{ player_daily : aggregates
    apps ||--o{ price_snapshots : priced
    apps ||--o{ release_events : scheduled
    apps ||--o{ game_documents : indexed
    game_documents ||--o{ game_embeddings : embedded
    apps ||--o{ recommendation_items : ranked
    recommendation_runs ||--o{ recommendation_items : contains
    anonymous_users ||--|| user_preferences : owns
    anonymous_users ||--o{ feedback_events : creates
    apps ||--o{ feedback_events : receives
    apps ||--o{ curation_overrides : corrected
```

## 7. 核心表

### 7.1 目录与关系

#### `apps`

| 字段 | 说明 |
| --- | --- |
| `app_id` PK | Steam AppID |
| `app_type` | game, demo, playtest, tool, dlc, unknown |
| `canonical_name` | 当前默认名称 |
| `release_state` | released, upcoming, coming_soon, retired, unknown |
| `release_date` | 精确日期可用时填写 |
| `release_date_raw` | 来源原始日期文本；模糊季度/月/年不得伪造成日级日期 |
| `release_date_precision` | day, month, quarter, year, tba |
| `is_early_access` | 可空布尔 |
| `current_data_confidence` | 当前记录综合可信度 |
| `source_modified_at_ms` | 来源报告的更新时间 |
| `created_at_ms`, `updated_at_ms` | MPGS 时间 |

索引：`release_state, release_date`、`app_type`、`updated_at_ms`。

#### `app_relations`

关系：`demo_of`、`playtest_of`、`dedicated_server_for`、`edition_of`、`replaces`。

唯一键：`source_app_id, target_app_id, relation_type`。保存 `confidence`、`evidence_id` 和 `verified_by_human`。

#### `app_localizations`

唯一键：`app_id, language`。保存本地化名称、短描述和来源，不在 `apps` 中堆积多语言列。

### 7.2 多人特征

#### `multiplayer_profiles`

每个基础游戏一条当前有效画像：

```text
dominant_mode
connection_methods_json
server_dependency
join_methods_json
progression_type
min_players
recommended_min_players
recommended_max_players
hard_max_players
private_session
online_coop
self_hosted_server
drop_in_out
crossplay
service_status
profile_confidence
computed_at_ms
```

集合字段 MVP 可用受校验 JSON 数组；用于高频过滤的字段必须单独成列。

#### `app_availability`

每个游戏最多一条推荐约束记录：平台与语言 JSON 数组、典型局时长上下界、免费状态和更新时间。平台来自 Steam `appdetails.platforms` 的结构化字段；语言由商店字符串归一化为 Steam 语言代码；典型局时长属于人工校准字段。人工覆盖平台/语言时保留商店证据，撤销后恢复最新来源值。

#### `app_media`

每个 App 至多一行列表封面（`capsule_url`）。由 catalog 种子、`appdetails.header_image` 或回填脚本维护，供 Feed/搜索/卡片使用。

#### `app_media_assets`（migration `0016_steam_media_gallery`）

一对多商店媒体快照，仅服务游戏详情画廊：

```text
PRIMARY KEY (app_id, kind, source_id)
kind ∈ {screenshot, movie}
sort_order
title (movie 可选)
thumbnail_url  -- screenshot 缩略图 / movie 海报
full_url       -- screenshot 大图；movie 必须为 NULL
mp4_url / hls_h264_url / dash_h264_url  -- movie 播放地址（至少一个非空）
is_highlight
source
updated_at_ms
```

入库语义（与 `appdetails` 同一事务）：

- `screenshots` / `movies` 字段为 `None`：保留该 kind 旧行。
- 为 `Some(items)`（含空数组）：删除该 kind 后插入新集合。
- 解析失败、请求失败或 `success=false` 不得清空旧媒体。
- 不下载二进制；只存白名单 URL。截图上限 20、视频上限 5。
- 旧库升级后表为空属正常；`app_media` 封面必须继续可用。
- `latest_data_update_ms` 包含 `app_media_assets.updated_at_ms`。

#### `app_media_backfill_state`（migration `0017_media_backfill_state`）

有界媒体补全账本（只服务联机富化候选）：

```text
app_id PK
attempts
last_attempt_at_ms
status ∈ {pending, complete, none, failed, exhausted}
updated_at_ms
```

Worker 策略（可用环境变量覆盖）：

- 当候选媒体覆盖率 **低于** `MPGS_MEDIA_BACKFILL_COVERAGE_THRESHOLD`（默认 `0.95`）时，才把缺媒体 App 排进 store 重拉队列。
- 每 App 最多 `MPGS_MEDIA_BACKFILL_MAX_ATTEMPTS`（默认 3）次；冷却 `MPGS_MEDIA_BACKFILL_COOLDOWN_MS`（默认 6h）。
- store 成功且已有 `app_media_assets` → `complete`；成功但无可用媒体 → `none`（停止）；失败 → `failed` / 达上限 `exhausted`。
- `MPGS_MEDIA_BACKFILL_ENABLED=false` 可关闭。仍不下载二进制，只再拉 appdetails 元数据。

#### `feature_evidence`

```text
evidence_id PK
app_id
feature_name
value_json
source_type
source_ref
source_document_id nullable
confidence
observed_at_ms
expires_at_ms nullable
is_active
```

对 `app_id, feature_name, is_active` 建索引。证据不会因当前值改变而原地覆盖，历史记录保留用于审计。

#### `curation_overrides`

保存人工覆盖值、原因、外部证据、操作者、创建/撤销时间。有效值解析时人工覆盖优先，但撤销后回到当前最佳来源值。

### 7.3 时间序列

#### `review_snapshots`

唯一键：`app_id, region_scope, language_scope, captured_at_ms`。

保存 `total_positive`、`total_negative`、`total_reviews`、Steam score、Wilson 值、是否过滤 off-topic activity 和来源参数哈希。

#### `player_snapshots`

保存单次 CCU。唯一键：`app_id, captured_at_ms`。原始高频记录按保留策略清理。

#### `player_daily`

保存 UTC 日聚合：最小、最大、中位近似、均值、样本数、缺失率。推荐器优先读取该表和近期滚动聚合。

#### `price_snapshots`

唯一键：`app_id, country_code, currency, captured_at_ms`。保存原价、现价、折扣、是否可购买和套餐标识。

#### `release_events`

保存发售日期每次变化，包含旧值、新值、精度、来源和观察时间。当前日期同时物化到 `apps`。

### 7.4 来源与任务

#### `source_documents`

保存清洗文本或短期原始响应：

```text
document_id
source
entity_type
entity_key
content_type
content_hash
content_text_or_blob
fetched_at_ms
expires_at_ms
parse_version
```

含个人评论文本的文档保留期更短；用于长期推荐的应是聚合主题和证据片段，而不是无限保存全文。

#### `source_cursors`

保存分页游标、`if_modified_since`、ETag、上次成功和下次运行时间。

#### `source_runs`

每次采集运行保存状态、请求数、成功数、错误类别、限流消耗和解析器版本。

#### `jobs`

保存任务、优先级、尝试次数、租约持有者、租约到期时间、入队幂等键和完成幂等键。MVP 不引入外部消息队列。

### 7.5 检索与 AI

#### `game_documents`

一个游戏可有多个受控文档块：`identity`、`store_summary`、`multiplayer_profile`、`review_topics`、`curation_notes`。保存内容哈希、语言、可见范围和更新时间。

#### `game_fts`

FTS5 虚拟表，索引标题、别名、标签和可检索文本。使用外部内容表或显式同步任务，避免隐藏触发器逻辑难以排错。

#### `game_embeddings`

```text
document_id
provider
model
dimensions
vector_blob
is_l2_normalized
content_hash
created_at_ms
```

唯一键：`document_id, provider, model, content_hash`。向量以明确端序的 `float32` BLOB 保存；读取时校验字节长度等于 `dimensions * 4`。

#### `ai_analyses`

保存离线特征提取结果和验证状态。原始模型输出与已接受的结构化特征分离，未验证输出不能进入当前多人画像。

#### `ai_analysis_cache`

保存在线/离线请求缓存键、模型、提示词版本、输入哈希、有效输出、用量和过期时间。

### 7.6 用户与推荐

#### `anonymous_users`

服务端使用系统随机源生成内部用户 ID、访问令牌和刷新令牌。令牌只保存不可逆哈希，并分别保存访问/刷新过期时间；刷新时同时轮换两种令牌。

#### `user_preferences`

每用户一条当前偏好，并带 `version` 做乐观并发控制。枚举与数值范围由 API 和数据库共同验证。

Migration `0019_preference_confidence` 追加 `preference_confidence REAL NOT NULL CHECK (0..1)`。它区分“领域默认值已经存在”与“玩家实际确认过这组偏好”：

- 升级前已有行因迁移兼容默认值保留 `1.0`，避免升级后突然把真实历史偏好收缩为中性。
- 新建用户由领域层显式写入 `0.0`；人数、模式、平台、开服、时长、预算和语言的持久化适配分均按该值向 `0.5` 收缩。
- `rules-0.3` 客户端应显式携带并持久化 `preference_confidence`；数据库不会根据一次普通字段更新自动猜测玩家是否已确认。服务端兼容期会检测旧客户端 PUT 是否省略该字段，省略时从当前行恢复原值后再更新，因此不会把迁移后 `1.0` 意外降为未确认；显式提交仍按请求值更新。
- 请求级显式硬条件通过独立的 `HardConstraints` 掩码生效，只把对应维度视为本次高置信条件，不修改该列。

该迁移仅追加一列，没有自动回写玩家行为标签，也没有启用跨游戏偏好学习。

#### `feedback_events`

追加式事件表：`like`、`not_interested`、`played`、`too_competitive`、`party_size_mismatch`、`hosting_friction`。完整请求指纹用于幂等冲突校验；撤销使用唯一追加事件，不删除历史。

#### `recommendation_runs`

Migration `0018_recommendation_telemetry`。保存 run ID、可选的部署密钥 HMAC `subject_hash`、请求类型/分区、算法和配置版本、评分语义、结构化上下文 SHA-256、候选集 SHA-256、候选数及创建/过期时间。禁止保存自然语言查询原文；账号 ID 的裸 SHA 不可作为 `subject_hash`。

#### `recommendation_items`

以 `(recommendation_run_id, app_id)` 为主键，保存 run 内唯一最终名次、内部相关性分、可空推荐指数、资料置信度、slot reason、受控分项 JSON 及记录/过期时间。

#### `recommendation_events`

只追加的曝光/交互归因事件，必须以复合外键指向真实的 recommendation item。公开接口仅准入 `exposure`、`detail_open`、`steam_click`、`play_intent`；归因反馈使用受控反馈类型。run 内幂等键防止重复；metadata 仅允许枚举、数值和受控标签，禁止查询原文、用户名、令牌和自由文本。迟到事件会把其关联 item/run 的过期时间延长到事件后的 90 天。Web 已采集这四类事件，但曝光仍以卡片挂载近似、Demo 无独立枚举，不能仅凭表和接口存在声称漏斗已完全覆盖。

### 7.7 算法配置

#### `algorithm_configs`

保存不可变版本化 JSON、Schema 版本、创建者、创建时间和状态。只有一个版本可标记为当前生产配置；当前配置会在读取时反序列化并校验，并驱动分区天数、CCU/Wilson/熟人适配门槛、候选上限和 MMR 参数。切换必须写审计事件。

配置版本与推荐器代码版本是两个维度：API 的 `algorithm_version` 来自正在执行的代码（当前 `rules-0.3.1`），`config_version` 来自本表的活动行。升级旧数据库时允许继续加载经校验的 `rules-0.2.0` 或 `rules-0.3.0` 配置，同时执行 `rules-0.3.1` 公式；不得把活动配置版本冒充代码版本。

## 8. SQLite 配置

MVP 初始策略：

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA trusted_schema = OFF;
```

- `journal_mode` 在初始化阶段设置并验证实际返回值。
- `synchronous=FULL` 优先保护用户反馈与人工校正；只有基准和恢复测试证明可接受后才考虑 `NORMAL`。
- 文件数据库为每个阻塞查询建立短生命周期只读连接，写连接由 Storage 层单写锁协调；内存测试库使用同一连接。
- `busy_timeout` 不能替代短事务和写入调度。
- 定期 checkpoint，并监控 WAL 文件大小；不能在每次请求后强制 checkpoint。

## 9. 迁移

- 迁移文件名称：`NNNN_description.sql`。
- 每个正式版本只能向前迁移，不能在已发布迁移中修改 SQL。
- 服务启动时只有 `migrate` 角色可执行迁移；其他角色检查版本并在不兼容时拒绝 ready。
- 破坏性迁移采用“新增列/表 -> 双写/回填 -> 切读 -> 后续版本清理”。
- 每个迁移在空库、上一版本副本和包含代表性数据的测试库上验证。
- 当前最新版本为 `0019_preference_confidence`。正式数据库仍须由用户确认后运行 migrator；代码库中存在迁移文件不等于某个部署已经升级。

## 10. 保留策略

初始值：

| 数据 | 保留期 |
| --- | --- |
| CCU 原始 30 分钟快照 | 90 天 |
| CCU 日聚合 | 长期 |
| 评论/价格快照 | 每日长期；高频记录 180 天后降采样 |
| 原始商店响应 | 30 天，必要证据片段长期 |
| 原始评论文本 | 默认不长期保存，最长 30 天用于聚合 |
| 推荐运行、条目与归因事件 | 90 天；迟到事件按其时间延长关联 run/item |
| AI 在线请求缓存 | 7～30 天，取决于是否含用户偏好 |
| 人工校正与审计 | 长期 |

清理任务必须按批次删除并避免长事务。

## 11. 备份与恢复

- 每日一致性全量备份；根据更新量增加更频繁备份。
- 使用 Online Backup API 或停止写入后的受控快照，不复制活动中的主文件/WAL 组合。
- 备份加密与访问控制由部署层处理，密钥不与备份放在一起。
- 保留多代备份，并定期在独立临时目录执行恢复演练。
- 恢复验收：`integrity_check`、迁移版本、关键表行数、黄金 AppID、FTS 重建和推荐冒烟测试。
- Embedding、FTS 和推荐快照可在恢复后重建，不应阻塞权威数据恢复。

## 12. 客户端缓存数据库

客户端 SQLite 只保存：

- 推荐流和详情响应缓存。
- 偏好副本与服务端版本号。
- 待同步反馈及幂等键。
- 图片缓存索引，不一定保存图片本身。

服务端 Schema 与客户端 Schema 不共享迁移文件。客户端缓存可在不影响用户偏好和待同步反馈的前提下重建。

## 13. 数据质量检查

定时检查：

- Demo/Playtest 关系循环或指向非基础游戏。
- `recommended_min > recommended_max` 等人数错误。
- 已发售游戏仍为未来日期，或日期精度与值冲突。
- 评论总数回退、CCU 长期缺样、价格币种不一致。
- 当前有效值没有任何活动证据。
- AI 特征引用不存在或已过期的文档。
- 人工覆盖与官方新数据冲突，需要复核而不是自动覆盖。

检查结果进入内部审核队列，并计入数据健康指标。
