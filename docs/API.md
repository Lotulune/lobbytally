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

访问令牌通过 `Authorization: Bearer <token>` 发送。公开目录端点可允许无令牌读取，但偏好、反馈和 AI 配额必须关联匿名会话。

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
- 响应明确 `data_updated_at_ms` 和 `algorithm_version`，避免把缓存时间当作数据时间。

## 6. 健康与元数据

### `GET /health/live`

只表示进程可响应，不检查外部依赖。

### `GET /health/ready`

检查迁移版本、数据库可读、当前算法配置和最小目录快照。AI/Steam 暂时不可用不应使前台 API 不 ready。

### `GET /v1/meta`

```json
{
  "api_version": "v1",
  "service_version": "0.1.0",
  "algorithm_version": "rules-0.2.0",
  "schema_version": 7,
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

`coop_competitive=0` 表示纯合作偏好，`1` 表示强竞技偏好。响应返回递增后的 `version`。

## 8. 推荐流

### `GET /v1/feeds/{section}`

`section`：

- `recent_release`
- `upcoming`
- `popular_legacy`
- `classic_legacy`

查询参数：

```text
limit, cursor, party_size, platforms, languages, session_minutes_min,
session_minutes_max, max_price_minor, currency, demo_only
```

`platforms` 与 `languages` 使用逗号分隔。查询参数覆盖当前请求的持久化偏好但不写回；已知平台、语言、时长或同币种价格不满足时硬过滤，候选数据未知时不等同于不支持。`demo_only=true` 仅保留 Demo/Playtest 或存在已知 Demo/Playtest 关系的游戏。

响应条目：

```json
{
  "app_id": 548430,
  "name": "Deep Rock Galactic",
  "section": "classic_legacy",
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
  "components": {
    "friend_fit": 0.92,
    "section_score": 0.90,
    "personalized_score": 0.91,
    "final_score": 0.91
  },
  "algorithm_version": "rules-0.2.0"
}
```

外层响应同时包含 `next_cursor`、`snapshot_at_ms`、`data_updated_at_ms` 和 `algorithm_version`。

## 9. 发售日历

### `GET /v1/calendar`

```text
?state=upcoming&from=2026-07-01&to=2026-12-31
```

`state` 必须是 `recent` 或 `upcoming`，省略时默认为 `upcoming`。`from/to` 最大跨度一年。日期不精确的条目进入 `undated_items`，不能伪造具体日期。每个条目包含 `release_date_precision`、`source_modified_at_ms`、`review_total` 和布尔型 `early_data`；`early_data` 由评论数量判断，不使用来源置信度代替评论成熟度。

```json
{
  "dated_items": [],
  "undated_items": [],
  "data_updated_at_ms": 1783936800000
}
```

## 10. 搜索

### `GET /v1/search`

用于名称和普通全文搜索：

```text
?q=deep+rock&party_size=4&limit=20
```

不调用在线 AI。可使用 FTS 和确定性排序。

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
    "include_demos": false,
    "selected_section": "popular_legacy"
  },
  "items": [],
  "ai_status": "fallback",
  "fallback_reason": "AI provider is not configured; deterministic intent parsing was used",
  "algorithm_version": "rules-0.2.0",
  "data_updated_at_ms": 1783936800000
}
```

`query` 长度为 3–500 个字符，公开 `limit` 为 3–10；服务端内部固定保留 Top 20 用于 hybrid/AI 二次分析，完成校验与融合后再截断。当前实现从自然语言中确定性解析人数、时长、合作/竞技倾向、平台、Demo 和自建服意愿，并可用 hybrid 检索重排（`hybrid_score`）。`ai_status` 取值：

| 值 | 含义 |
| --- | --- |
| `used` | 本次调用了 Provider 且校验通过 |
| `cached` | 命中服务端 AI 分析缓存 |
| `fallback` | Provider 失败/未配置等，确定性结果仍返回 |
| `disabled` | 空结果等边界下明确标记未启用路径 |

默认无外部 AI 时返回 `fallback` 与非空 `fallback_reason`（HTTP 200，兼容既有验收）。配置 `MPGS_AI_PROVIDER=openai_compat` 后，校验通过则 `used`/`cached`，并可能附加 `ai_summary`、`ai_summary_evidence_ids` / `ai_reasons`；用户可见 AI 文本缺少合法 evidence 时整次增强回退。

## 12. 游戏详情与证据

### `GET /v1/games/{app_id}`

返回：

- 本地化基础信息、商店链接和关联 Demo。
- 联机画像、人数、连接方式、服务依赖和可信度。
- 生命周期/近期评价、7/28 日 CCU 聚合和价格。
- 推荐分项、用户适配项、风险与更新时间。
- `play_intent`：社区「想玩」票数 `count` 与当前用户是否已投 `voted`（`voted` 需携带令牌，匿名请求恒为 `false`）。

当前响应的 `availability` 包含 `platforms`、`languages`、典型局时长范围、免费状态、最新价格/币种和 `has_demo`。缺失值返回空数组或 `null`，客户端不得解释为明确不支持。

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
  "recommendation_run_id": "019...",
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

### `POST /v1/feedback/{feedback_id}/undo`

追加撤销事件，不物理删除原记录；重复撤销返回同一撤销事件。有效反馈会参与后续推荐，撤销后立即退出推荐上下文。

## 13.1 游玩意愿（社区投票）

### `POST /v1/games/{app_id}/play-intent`

需携带令牌。请求体为 `{"intent": true}` 投票、`{"intent": false}` 撤票；同一 `(用户, AppID)` 至多一票，重复提交同一 `intent` 幂等。响应：

```json
{ "app_id": 548430, "count": 13, "voted": true }
```

票数是**跨用户的社区人气信号**，区别于个人 `feedback`。它作为版本化排序信号进入确定性推荐器（`algorithm_configs` 的 `play_intent_weight` / `play_intent_saturation`，`rules-0.2.0` 起启用）：票数越多，最终排序分越高，但有界且不覆盖硬过滤。0 票时信号为零、不改变既有排序。推荐流与游戏详情的响应含 `play_intent`；每次实际投票变更递增持久化 revision，使对应缓存 `ETag` 变化，并使基于旧排序的推荐流游标失效。限流并入反馈桶（每设备 60/min）。未知 AppID 返回 `404`。

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
POST   /admin/v1/games/{app_id}/overrides    # M2：创建人工覆盖
POST   /admin/v1/overrides/{id}/revoke       # M2：撤销覆盖
GET    /admin/v1/algorithms                  # 未实现
POST   /admin/v1/algorithms/{version}/activate
POST   /admin/v1/golden-tests/run
```

所有写操作记录操作者、原因、前后值和请求 ID（`x-request-id` 可选）。算法激活前必须有黄金测试结果。

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
