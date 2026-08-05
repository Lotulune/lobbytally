# HTTP API 契约

## 1. 约定

- Base path：`/v1`
- 传输：HTTPS；本地开发可使用 HTTP。
- 编码：UTF-8 JSON；时刻使用 Unix 毫秒并以 `_at_ms` / `_expires_at_ms` 结尾，日历日期使用 `YYYY-MM-DD`。
- 字段命名：`snake_case`。
- 未知响应字段客户端必须忽略；删除或改变字段语义需要新 API 版本。
- OpenAPI 3.1 由处理器注解和 Rust Schema 生成，运行时地址为 `GET /openapi.json`；本文解释语义，生成文档与契约测试约束机器可读结构。
- 所有响应返回 `x-request-id`。

## 2. 鉴权

### 2.1 匿名用户

首次调用 `POST /v1/session/anonymous`，服务端返回短期访问令牌和轮换令牌。服务端只保存令牌哈希。

```json
{
  "access_token": "opaque",
  "expires_at_ms": 1784023200000,
  "refresh_token": "opaque",
  "refresh_expires_at_ms": 1786615200000,
  "user_id": "u_opaque"
}
```

访问令牌通过 `Authorization: Bearer <token>` 发送。公开目录端点可允许无令牌读取；匿名会话仍可获得确定性推荐，但反馈写入和付费 AI 调用要求账号身份，AI 配额按账号及实际上游调用计数。

### `POST /v1/session/refresh`

请求体为 `{"refresh_token":"opaque"}`。成功后同时轮换访问令牌和刷新令牌，旧的两种令牌立即失效。

### 2.2 管理用户

管理 API 使用独立身份系统和 audience，不接受匿名用户令牌。MVP 可先使用部署层提供的管理员凭据，但必须支持审计到具体操作者。

## 3. 通用错误

```json
{
  "error": {
    "code": "invalid_argument",
    "message": "party_size must be between 1 and 64",
    "request_id": "019...",
    "details": {
      "field": "party_size"
    }
  }
}
```

稳定错误码：

| HTTP | code | 说明 |
| --- | --- | --- |
| 400 | `invalid_argument` | 输入格式或范围错误 |
| 401 | `unauthenticated` | 无有效令牌 |
| 403 | `forbidden` | 权限不足 |
| 404 | `not_found` | AppID 或资源不存在 |
| 409 | `version_conflict` | 偏好版本或幂等请求冲突 |
| 422 | `unsupported_constraint` | 输入合法但 MVP 不支持该约束 |
| 429 | `rate_limited` | 超过设备/IP/全局配额 |
| 500 | `internal` | 未分类内部错误 |
| 503 | `temporarily_unavailable` | 数据库迁移、只读降级或无可用数据 |

AI 失败通常不返回 5xx，而是以成功响应中的 `ai_status=fallback` 表达。

## 4. 游标分页

请求：

```text
?limit=20&cursor=<opaque>
```

响应：

```json
{
  "items": [],
  "next_cursor": "opaque-or-null",
  "snapshot_at_ms": 1783944000000
}
```

游标绑定分区、数据快照、完整偏好/反馈上下文、游玩意愿投票 revision 和偏移；目录、规则、偏好或投票变化后旧游标返回 `409 cursor_stale`。格式错误返回 `400`。客户端必须将游标视为不透明值。`limit` 默认 20，最大 100。

## 5. 缓存与一致性

- 推荐流、游戏详情、日历和元数据返回 `ETag`。
- 客户端可发送 `If-None-Match`，服务端返回 `304`。
- 偏好更新使用 `version` 乐观并发控制。
- 反馈写入支持 `Idempotency-Key`，相同键与相同请求返回原结果；相同键与不同请求返回 `409`。
- 响应明确 `data_updated_at_ms`、代码 `algorithm_version` 和活动 `config_version`，避免把缓存时间当作数据时间或把参数版本误作公式版本。

## 6. 健康与元数据

### `GET /.well-known/mpgs`

桌面客户端只持有服务 Origin 时使用的无认证发现端点。该端点不查询数据库，因此即使服务尚未 ready，也能识别 MPGS 服务并返回后续相对路径：

```json
{
  "service": "mpgs-server",
  "discovery_version": 1,
  "service_version": "0.1.0",
  "api_version": "v1",
  "api_base_path": "/v1",
  "readiness_path": "/health/ready",
  "openapi_path": "/openapi.json",
  "authentication": ["anonymous", "account"]
}
```

客户端必须验证 `service`、`discovery_version` 和 `api_version`，忽略未知字段，并始终在用户输入且已验证的 Origin 上解析相对路径。不得接受发现响应把请求切换到另一个 Origin。完整连接流程见 [C/S 架构强化 PRD](PRD_CS.md)。

### `GET /health/live`

只表示进程可响应，不检查外部依赖。

### `GET /health/ready`

检查迁移版本、数据库可读、当前算法配置和最小目录快照。AI/Steam 暂时不可用不应使前台 API 不 ready。

### `GET /v1/meta`

```json
{
  "api_version": "v1",
  "service_version": "0.1.0",
  "algorithm_version": "rules-0.3.1",
  "config_version": "rules-0.2.0",
  "schema_version": 19,
  "build_git_sha": "unknown",
  "data_updated_at_ms": 1783936800000,
  "supported_sections": [
    "recent_release",
    "upcoming",
    "popular_legacy",
    "classic_legacy"
  ],
  "ai_available": false,
  "storage_enabled": true
}
```

M6 起 `schema_version` / `build_git_sha` / `data_updated_at_ms` 用于发布物可追溯：`build_git_sha` 来自编译期 `MPGS_BUILD_GIT_SHA`（见 `apps/server/build.rs` 与 `scripts/package_server.ps1`）；本地未注入时为 `unknown`。

## 7. 偏好

### `GET /v1/preferences`

### `PUT /v1/preferences`

```json
{
  "version": 3,
  "preference_confidence": 1.0,
  "party_size": 4,
  "coop_competitive": 0.15,
  "session_minutes_min": 30,
  "session_minutes_max": 180,
  "budget_currency": "CNY",
  "budget_max_each_minor": 15000,
  "platforms": ["windows"],
  "self_hosting_willingness": 0.7,
  "languages": ["schinese", "english"],
  "excluded_modes": ["mmo"]
}
```

`coop_competitive=0` 表示纯合作偏好，`1` 表示强竞技偏好。`preference_confidence` 取值 `0..1`：`0` 表示尚未确认的初始化默认值，个性化各维收缩到中性 `0.5`；`1` 表示玩家已确认这组持久化偏好。Migration `0019_preference_confidence` 为兼容升级前已经存在的行写入 `1.0`，而服务端新建偏好的领域默认值为 `0.0`。本次 Feed/NL 查询中显式给出的限制只对对应维度立即按高置信处理，不会改写持久化值。响应返回递增后的 `version`。

`rules-0.3` 客户端应从 GET 结果显式原样回传 `preference_confidence`。兼容期内，服务端会在解析 PUT 前检测该字段是否存在；旧客户端省略时重新读取并保留数据库当前值，而不是应用领域反序列化默认 `0.0`。只有请求显式携带该字段时才能改变确认置信度；该兼容行为已有 HTTP 集成测试覆盖。

## 8. 推荐流

### `GET /v1/feeds/{section}`

`section`：

- `recent_release`
- `upcoming`
- `popular_legacy`
- `classic_legacy`

查询参数：

```text
limit, cursor, page, party_size, platforms, languages, session_minutes_min,
session_minutes_max, max_price_minor, currency, demo_only,
sort=recommended|fit_index|ccu|reviews|release_date,
order=asc|desc
```

`platforms` 与 `languages` 使用逗号分隔。查询参数覆盖当前请求的持久化偏好但不写回；已知平台、语言、时长或同币种价格不满足时硬过滤，候选数据未知时不等同于不支持。`demo_only=true` 仅保留 Demo/Playtest 或存在已知 Demo/Playtest 关系的游戏。

`sort` 在推荐打分与硬过滤之后重排结果：`recommended`（默认，保持多样性/探索编排）、`fit_index`（严格按内部连续 `relevance_score`）、`ccu`（在线人数）、`reviews`（评论数）、`release_date`（发售日）。`relevance` 与 `fit` 是 `fit_index` 的输入别名，响应统一回显 `fit_index`。`order` 为 `asc`/`desc`；未指定时 `release_date` 默认升序，其余标量排序默认降序。响应始终回显 `sort`，仅标量排序回显实际 `order`。

`recommended` 是经过个性化、多样性和探索处理的编排顺序，不是单一标量排序。该模式忽略传入的 `order`，响应将 `order` 设为 `null`；客户端不得为它显示升/降序按钮。`fit_index` 是查看纯适配相关性顺序的严格标量入口，不保留 MMR 编排。CCU、评论和日期排序中，缺失值无论方向都排在有值条目之后。

响应条目：

```json
{
  "app_id": 548430,
  "name": "Deep Rock Galactic",
  "section": "classic_legacy",
  "rank": 1,
  "recommendation_index": 94,
  "fit_band": "excellent",
  "data_confidence": 0.92,
  "friend_fit": 0.95,
  "slot_reason": "base",
  "score_calibration_version": "context-percentile-v1",
  "score": 0.91,
  "confidence": 0.92,
  "party": {
    "recommended_min": 1,
    "recommended_max": 4
  },
  "multiplayer": {
    "dominant_mode": "private_coop"
  },
  "play_intent": {
    "count": 12,
    "voted": false
  },
  "reasons": ["支持私人四人合作", "累计口碑稳定"],
  "cautions": ["高难度任务需要配合"],
  "evidence_ids": ["feature:online_coop:548430"],
  "reason_evidence": ["feature:online_coop:548430"],
  "feature_freshness": {
    "multiplayer": {"status": "fresh", "observed_at_ms": 1785686400000},
    "reviews": {"status": "fresh", "observed_at_ms": 1785686400000},
    "activity": {"status": "unknown", "observed_at_ms": null},
    "price": {"status": "unknown", "observed_at_ms": null},
    "release": {"status": "fresh", "observed_at_ms": 1785686400000}
  },
  "components": {
    "friend_fit": 0.95,
    "section_score": 0.90,
    "personalized_score": 0.91,
    "group_fit": 0.96,
    "mode_fit": 0.90,
    "access_fit": 0.88,
    "hosting_fit": 0.93,
    "session_fit": 0.75,
    "quality": 0.89,
    "activity": 0.82,
    "freshness": 0.20,
    "risk": 0.04,
    "relevance_score": 0.91,
    "final_score": 0.91
  },
  "algorithm_version": "rules-0.3.1"
}
```

`algorithm_version` 表示当前执行的代码/公式版本，`config_version` 表示数据库活动阈值与参数版本。升级旧数据库时 `algorithm_version="rules-0.3.1"` 与 `config_version="rules-0.2.0"` 可以同时出现，这是兼容行为，不表示仍在执行旧算法。新部署默认配置版本为 `rules-0.3.0`。

字段语义：

- `rank` 是当前完整排序中的一基最终名次，不是当前分页内序号。
- `recommendation_index` 是当前请求实际返回窗口（排序后 `skip(offset).take(limit)` 得到的条目）内，按连续 `relevance_score` 中位秩计算的相对推荐指数；`N` 等于该窗口实际返回的条目数。它不是整分区/全目录百分位或概率，客户端不得加 `%`。返回窗口少于 10、`data_confidence < 0.45` 或有效独立特征少于 3 时为 `null`。改变分页、`offset` 或 `limit` 可能改变指数；精确同分仍共享同一指数。
- `fit_band` 为 `excellent`、`good`、`consider` 或 `insufficient_data`。
- `data_confidence` 是多人画像证据覆盖、评论、活动与发售日期置信度的合成值，用于逐信号先验收缩和是否展示指数；它不是正向排名奖励。`friend_fit` 是熟人联机结构适配，两者不得互换。
- `slot_reason` 为 `base`、`diversity` 或 `explore`，用于解释多样性重排后指数与名次不严格单调的情况。
- `score` 是暂时保留的原始相关性兼容字段，不是概率；`confidence` 是 `data_confidence` 的兼容别名。新客户端应使用新字段。
- `components` 完整返回 `group_fit`、`mode_fit`、`access_fit`、`hosting_fit`、`session_fit`、`quality`、`activity`、`freshness`、`risk`，并保留旧分项。
- `reason_evidence` 返回本卡理由引用的规范证据 ID；兼容字段 `evidence_ids` 同步保留。AI 理由通过验证后，其证据 ID 会并入两者且去重。当前是卡片级平面集合，不声称与每一句理由一一配对。
- `feature_freshness` 分别公开 `multiplayer/reviews/activity/price/release` 的 `status=fresh|unknown` 与来源时间。动态信号已在查询层执行 TTL，因此过期值不会伪装成 `fresh`；`unknown` 的时间必须为 `null`。
- 排序内部使用规范 `ModeFamily`，但 `multiplayer.dominant_mode` 为兼容字段，仍可能返回 `pvp` 等历史展示值；客户端不能依赖该展示值复制服务端过滤规则。

外层响应同时包含 `next_cursor`、`snapshot_at_ms`、`data_updated_at_ms`、代码 `algorithm_version`、活动 `config_version`、`score_semantics="context_percentile_v1"`、`sort`、可空 `order` 和可空 `recommendation_run_id`。

服务端在返回 Feed 前保存 run 及当前页 items；成功时返回以 `rr_` 开头的 run ID。候选集哈希和 `candidate_count` 覆盖完整排序池，item 明细覆盖当前响应页。持久化失败时推荐仍可用，但 run ID 为 `null`；客户端不能自造 ID 或声称该页交互已成功归因。

## 9. 发售日历

### `GET /v1/calendar`

```text
?state=upcoming&from=2026-07-01&to=2026-12-31
```

`state` 必须是 `recent` 或 `upcoming`，省略时默认为 `upcoming`。未传 `from/to` 时，`upcoming` 默认查询今天至未来 60 天，`recent` 默认查询过去 180 天至今天；显式传入的日期最大跨度一年。日期不精确的条目进入 `undated_items`，不能伪造具体日期。每个条目包含 `release_date_precision`、`source_modified_at_ms`、`review_total` 和布尔型 `early_data`；`early_data` 由评论数量判断，不使用来源置信度代替评论成熟度。

```json
{
  "dated_items": [],
  "undated_items": [],
  "data_updated_at_ms": 1783936800000
}
```

## 10. 搜索

### `GET /v1/search`

用于名称搜索（**中英双名**）：

```text
?q=deep+rock&party_size=4&limit=20
```

匹配范围：

- `apps.canonical_name`（列表展示用主名，生产多为简中店面名）；
- `app_localizations.name` 中语言为 `schinese` / `english`（及 `en`）的字段。

因此简中主名游戏在补全英文本地化名后，可用英文原名搜到；Valheim 这类主名本身含中英时，两种片段都可命中。其它语言名暂不参与。大小写对 ASCII 不敏感（`COLLATE NOCASE`）。不调用在线 AI。

### `POST /v1/search/semantic`

用于自然语言混合检索，但不要求生成长解释：

```json
{
  "query": "三个人一小时左右、不太卷、可以反复刷",
  "limit": 20,
  "use_ai_intent_parser": true
}
```

Embedding 或 AI 意图解析不可用时回退到 FTS 和当前偏好。

## 11. AI 推荐

### `POST /v1/recommendations/natural-language`

```json
{
  "query": "四个人长期玩，能自己开服，优先 Windows",
  "limit": 6
}
```

响应：

```json
{
  "query": "四个人长期玩，能自己开服，优先 Windows",
  "interpreted": {
    "party_size": 4,
    "session_minutes_max": null,
    "coop_competitive": null,
    "self_hosting_willingness": 1.0,
    "platforms": ["windows"],
    "demo_only": false,
    "selected_section_explicit": false,
    "modes_preferred": ["self_hosted"],
    "applied_constraints": [
      "party_size",
      "platforms",
      "self_hosting_preference",
      "modes_preferred"
    ],
    "unapplied_constraints": []
  },
  "items": [],
  "ai_status": "fallback",
  "fallback_reason": "AI provider is not configured; deterministic intent parsing was used",
  "algorithm_version": "rules-0.3.1",
  "config_version": "rules-0.2.0",
  "recommendation_run_id": "rr_...",
  "score_semantics": "context_percentile_v1",
  "data_updated_at_ms": 1783936800000
}
```

`query` 长度为 3–500 个字符，公开 `limit` 为 3–10。服务端先对已同步的 FTS 与向量索引执行 `RRF(k=60)`，最多得到 300 个去重 AppID；再从每个适用分区的完整排名集合中准入这些命中。准入发生在分区资格、显式硬条件和负反馈之后，因此检索不能恢复已淘汰游戏。剩余容量按严格 `relevance_score` 补齐，四区去重联合池最多 300，随后执行可选的有界 AI 数值融合和一次全局 MMR，最后才截断到公开 `limit`。显式分区请求保持单区范围；检索不可用或为空时按确定性相关性补齐。

当前实现确定性解析人数、时长、合作/竞技倾向、平台、Demo、自建服意愿和排除模式。`modes_excluded` 进入硬过滤；可识别的 `private_coop` / `matchmade_pvp` / `self_hosted` 偏好分别映射为合作竞技倾向或自建服软分，无法忠实表示的模式进入 `unapplied_constraints`。`applied_constraints` / `unapplied_constraints` 是服务端对实际转换结果的回执，客户端不得仅凭原始自然语言声称条件已应用。检索索引由持久化游标循环覆盖 MPGS 本地目录；这不代表即时搜索整个 Steam 目录。`ai_status` 取值：

内部四个 Feed 召回不写 recommendation run。自然语言响应只在最终 RRF/AI/MMR 排序和排名元数据刷新后创建一个独立的 `request_kind=natural_language` run；通用请求的 run 分区记为 `all`，显式请求记为对应分区。候选哈希覆盖截断前联合池，item 明细覆盖实际返回项。run 上下文来自结构化 interpreted intent 的哈希，不保存 `query` 原文；写入失败时 `recommendation_run_id` 为 `null`，不影响推荐结果。

| 值 | 含义 |
| --- | --- |
| `pending` | 基础结果已返回，AI 增强仍在进行（渐进式搜索） |
| `used` | 本次调用了 Provider 且校验通过 |
| `cached` | 命中服务端 AI 分析缓存 |
| `fallback` | Provider 失败/未配置等，确定性结果仍返回 |
| `disabled` | 空结果等边界下明确标记未启用路径 |

响应还可包含 `ai_provider` 与 `ai_latency_ms`，用于显示本次 AI 阶段实际选择的 Provider 和耗时。`cached` 可能近乎即时返回；`fallback` 表示模型请求或输出校验失败，页面仍保留确定性推荐。

默认无外部 AI 时返回 `fallback` 与非空 `fallback_reason`（HTTP 200，兼容既有验收）。配置 `MPGS_AI_PROVIDER=openai_compat` 后，校验通过则 `used`/`cached`，并可能附加 `ai_summary`、`ai_summary_evidence_ids` / `ai_reasons`；用户可见 AI 文本缺少合法 evidence 时整次增强回退。

AI 返回的数组顺序没有排序权限。服务端只把合法候选的 `fit_score` 与 `confidence` 当作数值调整，优先使用当前完整的 `components.relevance_score`（包含社区小幅加成，旧响应才回退 `score`）计算最多 15% 的混合影响，再按混合分、原始位置和 AppID 稳定排序；候选成员集合保持不变，并重新计算 `rank` 与上下文推荐指数。AI 不能恢复硬过滤或负反馈隐藏的条目。

发送给 Provider 的候选摘要有严格字节上限，模型最多返回 8 个候选调整；其余候选保留确定性分数和稳定顺序。

内置 Provider 走 M8 多模型任务路由（`intent_parse` / `rank_explain` 等）：主模型与回退链由配置与 `/v1/models` 发现决定，支持 Chat Completions 与 Responses 双协议；用户自定义 Key 仍为单模型，不写入服务端路由表。

### `POST /v1/ai/search`

渐进式自然语言搜索。请求体：

```json
{
  "query": "三个人私密合作、Windows、预算 100 元",
  "limit": 6,
  "async": true
}
```

响应在自然语言推荐基础上增加 `analysis_id`。`async=true` 时客户端可轮询增强状态；基础候选始终可用。

### `GET /v1/ai/analyses/{analysis_id}`

读取渐进式分析状态与缓存结果。过期或不存在返回 `404`。

### `POST /v1/ai/compare`

输入 2–4 个候选 `app_ids`，服务端生成事实矩阵；模型只解释差异，不得使用任意列名。失败时仍返回事实矩阵。

```json
{ "app_ids": [548430, 632360] }
```

### `GET /v1/games/{app_id}/ai-summary`

读取六段式游戏总结。优先缓存/离线模型结果；否则返回规则摘要（`ai_status=fallback`）并落库待审核。

### `POST /v1/ai/group-advice`

仅接受聚合偏好、候选 AppID 与公开票数，不接受成员隐私字段。AI 失败时使用确定性折中排序。

```json
{
  "party_size": 3,
  "platforms": ["windows"],
  "candidate_app_ids": [548430, 632360],
  "vote_counts": [{ "app_id": 548430, "votes": 4 }]
}
```

### `GET|POST /admin/v1/bootstrap`

观察/启动首次启动模式（`store_only`、重点候选优先、Web 发现入队）。`POST` 需要管理员 Bearer Token。

自然语言请求可附 `async=true`（基础结果不等待 rank AI）与 `intent_delta`（结构化多轮增量，不传完整聊天原文）。

## 12. 游戏详情与证据

### `GET /v1/games/{app_id}`

返回：

- 本地化基础信息（含可选的 `short_description`）、封面、商店链接和关联 Demo。
- 联机画像、人数、连接方式、服务依赖和可信度。
- 生命周期/近期评价、7/28 日 CCU 聚合和价格。
- 推荐分项、用户适配项、风险与更新时间。
- `play_intent`：社区「想玩」票数 `count` 与当前用户是否已投 `voted`（`voted` 需携带令牌，匿名请求恒为 `false`）。

当前响应的 `availability` 包含 `platforms`、`languages`、典型局时长范围、免费状态、最新价格/币种和 `has_demo`。`reviews.total` / `positive` 是 Steam 全语言评价汇总；`reviews.featured` 为按 Steam `filter=all` 顺序同步的简体中文热门评价，最多 10 条，包含正文、推荐态度、公开作者名/主页、游玩时长、有用票数和撰写时间。正文会清理 Steam BBCode 并截断到 4,000 字符。缺失值返回空数组或 `null`，客户端不得解释为明确不支持。

向后兼容增量字段 `media`（始终存在于新服务端响应中；旧服务端可能省略，客户端应按空媒体处理）：

```json
{
  "media": {
    "updated_at_ms": 1784880000000,
    "screenshots": [
      {
        "id": "0",
        "thumbnail_url": "https://shared.akamai.steamstatic.com/...",
        "full_url": "https://shared.akamai.steamstatic.com/..."
      }
    ],
    "videos": [
      {
        "id": "257363622",
        "title": "1.0 Release Date Reveal Trailer",
        "poster_url": "https://shared.akamai.steamstatic.com/...",
        "highlight": true,
        "mp4_url": null,
        "hls_h264_url": "https://video.akamai.steamstatic.com/...",
        "dash_h264_url": "https://video.akamai.steamstatic.com/..."
      }
    ]
  }
}
```

- `media` 只出现在游戏详情；Feed、搜索、日历、社区列表**不**携带完整媒体数组。
- 无媒体时 `screenshots` / `videos` 为 `[]`，`updated_at_ms` 为 `null`（不是 `null` 数组）。
- URL 均经过服务端 Steam CDN 白名单；客户端不得用 AppID 猜测截图/视频 URL。
- 现有 `cover_url` / `cover_updated_at_ms` 保持不变；媒体刷新会抬升 `data_updated_at_ms`，从而使详情 ETag 变化。

### `GET /v1/games/{app_id}/evidence`

默认返回对最终推荐产生影响的公开证据摘要，不返回内部敏感备注。支持 `?feature=private_session`。

```json
{
  "items": [
    {
      "evidence_id": "feature:private_session:548430",
      "feature": "private_session",
      "value": true,
      "source_type": "official_store",
      "source_label": "Steam store feature",
      "confidence": 0.9,
      "observed_at_ms": 1783936800000
    }
  ]
}
```

## 13. 反馈

### `POST /v1/feedback`

请求必须包含 `Idempotency-Key`：

```json
{
  "app_id": 548430,
  "type": "like",
  "recommendation_run_id": "rr_...",
  "client_created_at_ms": 1783942200000
}
```

`type`：

- `like`
- `not_interested`
- `played`
- `too_competitive`
- `party_size_mismatch`
- `hosting_friction`

服务端读取活动反馈时把 `like/not_interested` 视为 sentiment、`played` 视为 ownership，其余原因各自独立；同一游戏的这些维度可以同时生效。公开写入仍保持单个 `type` 的兼容契约，尚未切换到一次提交 `sentiment + reason_tags[] + ownership` 的新 wire shape。

`recommendation_run_id` 是可选归因字段。提供时，服务端先验证 `(run_id, app_id)` 确实存在于 `recommendation_items`；不匹配返回 `400 invalid_argument`，不能把反馈错误归到其他曝光。反馈创建成功后，以同一幂等键把受控 `feedback_type` metadata 写入 `recommendation_events`。未提供 run ID 时反馈仍可保存，但不计为已归因事件。

### `POST /v1/feedback/{feedback_id}/undo`

追加撤销事件，不物理删除原记录；重复撤销返回同一撤销事件。有效反馈会参与后续推荐，撤销后立即退出推荐上下文。

## 13.1 游玩意愿（社区投票）

### `POST /v1/games/{app_id}/play-intent`

需携带令牌。请求体为 `{"intent": true}` 投票、`{"intent": false}` 撤票；同一 `(用户, AppID)` 至多一票，重复提交同一 `intent` 幂等。响应：

```json
{ "app_id": 548430, "count": 13, "voted": true }
```

票数是**跨用户的全站社区信号**，区别于个人 `feedback`，也不能称为“好友想玩”。`rules-0.3` 对它采用保守曲线：前 5 个不同账户不改变排序；之后按 `min(configured_weight, 0.03) × (count-5)/(count-5+20)` 加分，最大提升 3 个百分点。它不能恢复硬过滤或明确负反馈隐藏的条目；禁用权重或 saturation 为 0 时完全不影响排序。推荐流与游戏详情的响应含 `play_intent`；每次实际投票变更递增持久化 revision，使对应缓存 `ETag` 变化，并使基于旧排序的推荐流游标失效。限流并入反馈桶（每设备 60/min）。未知 AppID 返回 `404`。

## 13.2 推荐交互事件

### `POST /v1/recommendation-events`

请求必须包含 `Idempotency-Key`，并只接受已经出现在对应 run item 中的 AppID：

```json
{
  "recommendation_run_id": "rr_...",
  "app_id": 548430,
  "event_type": "detail_open",
  "client_created_at_ms": 1783942200000
}
```

`event_type` 仅允许：

- `exposure`
- `detail_open`
- `steam_click`
- `play_intent`

服务端先验证 `(recommendation_run_id, app_id)`，不匹配返回 `400 invalid_argument`；run 内相同幂等键与相同请求返回原记录，不同请求返回 `409`。成功响应为 `201`，包含 `recommendation_event_id`、run ID、AppID、`event_type`、可空客户端时间、受控 `metadata` 和服务端 `created_at_ms`。该接口只记录归因事件；`event_type=play_intent` 不会代替 `/v1/games/{app_id}/play-intent` 投票。

服务端契约和 Storage 已接线。Web 会把来源 run 保留到详情页，并尽力上报卡片挂载时的 `exposure`、打开卡片时的 `detail_open`、Steam 外链的 `steam_click`，以及带 run 上下文的成功想玩投票 `play_intent`。确定性幂等调用默认省略 `client_created_at_ms`，避免同一 key 重试时因时间变化变成不同 payload；只有调用方显式提供时才发送。想玩投票已成功但归因遇到可重试错误时，PlayIntentStore 保留 run 并在后续 flush 重试；永久无效或过期 run 只放弃归因，不撤销玩家投票。当前 `exposure` 尚未用可见区域观察器确认真实曝光，Demo 跳转也没有独立事件枚举，因此“客户端已写入”仍不能等同于“线上漏斗已完整采集”。

## 13.3 推荐归因存储契约

Migration `0018_recommendation_telemetry` 提供三个只追加表：

- `recommendation_runs`：请求类型/分区、算法与配置版本、评分语义、结构化上下文哈希、候选集哈希和候选数。
- `recommendation_items`：run 内 AppID、唯一名次、内部相关性分、可空推荐指数、资料置信度、slot reason 和分项 JSON。
- `recommendation_events`：必须外键指向真实 run item，使用 run 内幂等键，保存枚举事件、可选客户端时间和受控结构化 metadata。

默认保留期为 90 天。迟到事件会延长关联 item/run 的过期时间，使该事件仍拥有完整的 90 天归因窗口；清理按 `events → items → runs` 删除过期行。上下文只保存 SHA-256 哈希，不保存自然语言原文；`subject_hash` 必须是部署密钥参与的 HMAC/伪名，禁止使用可枚举账号 ID 的裸 SHA。`metadata_json` 只允许枚举、数值和受控标签，调用方不得写入自由文本、查询原文、用户名或令牌。

状态：migration、Storage 写入/读取/清理接口、Feed 与自然语言 run/item 写入、反馈验证/event 写入、公开受控交互 event 接口，以及 Web 的 run 透传、曝光/详情/Steam/想玩上报均已接线。曝光当前以卡片挂载近似，Demo 无独立事件。现有真实数据库仍需由 migrator 升级后才拥有这些表。

## 14. 同步

### `GET /v1/sync`

用于客户端增量获取偏好版本、已变更缓存实体和服务端建议失效列表：

```text
?since=<opaque_sync_cursor>
```

MVP 可以先按推荐流和详情分别使用 ETag；统一 sync 端点属于 P1，不能阻塞首个垂直切片。

## 15. 内部采集 API

内部路由使用 `/internal/v1`，不出现在公开客户端 OpenAPI 中。  
M2 最小实现要求 `Authorization: Bearer <MPGS_ADMIN_TOKEN>`（与管理 API 共用部署令牌；后续可拆分 audience）。

### `POST /internal/v1/jobs/enqueue`（M2）

入队采集任务；`idempotency_key` 唯一，重复提交返回已有 `job_id`。

### `POST /internal/v1/jobs/lease`（M2）

采集节点领取限定数量、可选 `source` 过滤的任务。请求体：`owner`、`limit`、`lease_ms`、`source`。

### `POST /internal/v1/jobs/{job_id}/complete`（M2）

验证租约持有者与幂等键后标记完成；同键重复完成返回成功。

### `POST /internal/v1/jobs/{job_id}/fail`（M2）

错误必须使用稳定类别：`network`、`rate_limited`、`auth`、`not_found`、`parse_changed`、`invalid_payload`。可重试错误按 `retry_delay_ms` 回到 `pending`；否则进入 `dead`。

## 16. 管理 API

管理路由使用 `/admin/v1`，Bearer 使用 `MPGS_ADMIN_TOKEN`。

```text
GET    /admin/v1/source-runs                 # 未实现（M3+）
GET    /admin/v1/review-queue                # 未实现（M3+）
GET    /admin/v1/games/{app_id}/debug        # M2：app + multiplayer_profile
GET    /admin/v1/data-status                  # M7：任务状态 + M3/M7 数据覆盖率
POST   /admin/v1/games/{app_id}/overrides    # M2：创建人工覆盖
POST   /admin/v1/overrides/{id}/revoke       # M2：撤销覆盖
GET    /admin/v1/algorithms                  # 未实现
POST   /admin/v1/algorithms/{version}/activate
POST   /admin/v1/golden-tests/run
```

所有写操作记录操作者、原因、前后值和请求 ID（`x-request-id` 可选）。算法激活前必须有黄金测试结果。

`GET /admin/v1/data-status` 返回每项维护任务的最近成功时间、下次运行、稳定错误类别、游标和 M3 覆盖率；新增的 `m7_coverage` 使用当前算法配置统计候选、可信熟人联机画像、日期、封面、四个分区和连续 7 天的评价/CCU 覆盖。它是 `mpgs-dbtool m7-data-audit` 的可观测对应物，不表示发布门禁已经通过。

## 17. 限流与大小限制

M3 默认值：

| 路由 | 限制 |
| --- | --- |
| 普通读取 | 每设备 120/min，叠加 IP 防滥用 |
| 普通搜索 | 每设备 30/min |
| 匿名会话创建/刷新 | 每设备/IP 20/min |
| AI 推荐 | 每设备 5/min、50/day，并受全局预算限制 |
| 反馈 | 每设备 60/min |
| 请求 JSON | 默认最大 64 KiB |
| AI 自然语言 query | 最大 2,000 Unicode 字符 |

普通读取、搜索、会话和反馈同时按 `x-device-id`（缺失时使用会话令牌）与客户端 IP 计数，并叠加默认 `10,000/min` 全局上限。只有 `MPGS_TRUST_PROXY_HEADERS=true` 时才信任 `X-Forwarded-For`/`X-Real-IP`；否则使用 TCP 对端地址。具体值由 `MPGS_RATE_LIMIT_*_PER_MINUTE` 调整。429 响应返回 `Retry-After`、`x-ratelimit-limit` 和 `x-ratelimit-remaining`。

M3 已实现默认 `64 KiB` 请求体上限和上述公开限流；AI 路由的日预算在 M5 Provider 接入时实现。

## 17.1 CORS（M4）

桌面客户端从 webview 源（Windows `http://tauri.localhost`，其他平台 `tauri://localhost`）跨源调用服务端，因此服务端维护一个精确源白名单：

- 默认允许 `http://tauri.localhost`、`tauri://localhost`、`http://localhost:5173`（浏览器/Tauri 开发）。
- `MPGS_CORS_ALLOWED_ORIGINS` 用逗号分隔的精确源覆盖默认值；每个源必须是 `scheme://host[:port]`（scheme 限 `http`/`https`/`tauri`，不含路径），非法值导致启动失败。
- `MPGS_CORS_ENABLED=false` 关闭 CORS（此时不返回任何 `Access-Control-Allow-Origin`）。
- 从不使用通配符 `*`，从不允许凭据（Bearer 走 `Authorization` 头，不用 Cookie）。
- 预检 `OPTIONS` 在鉴权与限流之前短路返回 `204`；未在白名单中的源不会收到 `Access-Control-Allow-Origin`，浏览器据此拦截，而非浏览器客户端不受影响。
- 允许方法 `GET, POST, PUT, OPTIONS`；允许请求头 `authorization, content-type, idempotency-key, if-none-match, x-device-id, x-request-id`；暴露响应头 `etag, x-request-id, retry-after, x-ratelimit-limit, x-ratelimit-remaining`。

## 18. 契约测试

- OpenAPI Schema 与 Rust DTO 快照一致。
- 每个错误码有示例和状态码测试。
- 旧客户端忽略新增字段的兼容测试。
- 游标篡改、过期和查询不匹配测试。
- 幂等键重复/冲突测试。
- AI `used/cached/disabled/fallback` 四种响应测试。
- 所有 AppID、价格、比例、人数和字符串长度边界测试。

## 19. M7 账号、社区与 AI 设置

### 19.1 账号会话

- `POST /v1/auth/register`：请求 `username`、`display_name`、`password`、可选 `device_label`，返回账号会话令牌和公开资料。
- `POST /v1/auth/login`：请求 `username`、`password`、可选匿名访问令牌和偏好冲突选择；不区分不存在账号和错误密码。
- `POST /v1/auth/refresh`、`POST /v1/auth/logout`、`POST /v1/auth/logout-all`：分别用于轮换、当前设备退出和全部设备退出。
- `PUT /v1/auth/password`：必须提供旧密码；成功后使其他刷新会话失效。
- `GET|PATCH|DELETE /v1/me`：读取/修改公开显示名称，或注销账号。`PUT|DELETE /v1/me/avatar` 仅允许 JPEG、PNG、WebP 的二进制上传，最大 2 MiB。

账户写操作必须使用账号令牌；匿名令牌仅可浏览和在登录时作为合并来源。响应不返回密码、令牌哈希、AI 原始密钥或内部用户标识。

### 19.2 社区投票

`GET /v1/community/play-intents?sort=trending|most_voted&limit=<1..100>&release_state=&demo_only=&platform=&party_size=&cursor=<opaque>` 返回独立于推荐流的社区列表。支持发售状态、Demo、`windows|macos|linux` 平台和 `1..64` 人数筛选。每项包含总票数、当前账号是否已投票，以及最多 5 个公开投票者头像；响应带 ETag，游标与排序、筛选和快照绑定。

`POST /v1/games/{app_id}/play-intent` 需要账号令牌。请求 `{"intent":true}` 投票、`{"intent":false}` 撤票；同一账号对同一 AppID 至多一票，重复提交幂等。

管理员可使用 `POST|DELETE /admin/v1/accounts/{user_id}/avatar/block` 屏蔽或解除当前头像，正文包含 `operator` 和 `reason`。屏蔽按内容哈希生效，公开头像回退默认图，并在 `audit_events` 留存操作理由。

### 19.3 AI 设置

- `GET|PUT /v1/me/ai-settings`：读写账号的 `builtin` 或 `off` 状态；自定义模式的 URL、模型与 API Key 由设备本地配置接管，服务端拒绝持久化 Key。
- `POST /v1/me/ai-settings/test`：使用请求中临时携带的 Key 探测 Provider 的 `GET /models`，请求完成后即丢弃，UI 和服务端日志均不记录响应正文或 Key。自定义 endpoint 必须是 HTTPS 公网地址，拒绝回环、私网、链路本地和重定向。
- `DELETE /v1/me/ai-settings/custom-key`：不可恢复地删除自定义凭据，并回退到内置或关闭模式。

内置 AI 的日额度按账号持久化计数，并受每账号并发限制和服务端全局预算约束；缓存命中不消耗额度。桌面端自定义 Key 保存在操作系统凭据库；浏览器预览保存在当前标签页的 `sessionStorage`，关闭标签页后清除。调用时 Key 仅随单次 HTTPS 请求进入服务端内存，不写入服务端 SQLite、缓存或日志。
