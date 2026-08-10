# 数据采集、推荐展示与运行可靠性优化 PRD

状态：PR A 可靠性热修已实施；PR B 生命周期采集与吞吐、PR C 推荐展示语义待实施。

基线日期：2026-08-10（Asia/Shanghai）。本文承接 `DATA_PIPELINE_NEXT_STEPS.md`
中已经完成的 PR 1/PR 2，不修改其历史验收结论。

## 1. 背景与结论

当前系统已经具备候选发现、分阶段一体化入库、推荐排序、自动部署和数据运维快照，
但新实现暴露出三类问题：

1. 未发售游戏、Demo/Playtest 和已发售游戏使用近似相同的富化路径，浪费评价、热门
   评价和 CCU 请求预算，拖慢真正需要完整数据的已发售游戏。
2. 推荐名次、适配程度和标量排序混在同一个徽章中。API 的 `rank` 在非推荐排序时只是
   当前列表位置，前端却仍把它显示成“第 N 推荐”。
3. PR #25/#28 的部分保护逻辑粒度不足：Worker 健康状态不可区分软过期与硬故障，
   空字段退避跨维度耦合，一体化队列没有 dead 终态。

本文的总体决策是：

- 以游戏生命周期决定采集深度，而不是让所有 App 走固定四阶段流水线。
- 未发售条目优先做到“基础资料可展示且发售状态及时”；已发售游戏优先做到“推荐证据
  完整且持续刷新”。
- 推荐顺序和推荐指数都严格使用相关性得分降序，避免多样性槽把低可靠度候选抬进头部。
- 推荐顺序只显示全局前 5 名；推荐指数排序显示全局前 10 名。其他标量排序不显示推荐名次。
- 可靠性修复先于吞吐调优。不能通过扩大批量或延长 watchdog 掩盖 poison item、错误
  健康状态或跨维度退避。

## 2. 目标与非目标

### 2.1 目标

- 减少未发售游戏和 Demo/Playtest 的无效 Steam 请求。
- 缩短新发现游戏从入队到“可展示基础资料”和“完整推荐资料”的等待时间。
- 保证发售状态变化后，条目能自动从基础采集升级到完整采集。
- 消除 CCU、评价数和发售日期排序中的伪推荐名次；推荐顺序突出前 5 名，推荐指数突出前 10 名。
- 为一体化入库提供有界重试、dead 可观测性和显式 requeue。
- 保证同 SHA 自动部署能忽略真正的软健康过期，但会恢复硬故障或卡死 Worker。
- 将空字段退避拆到数据维度，价格空值不得冻结发售状态和商店基础资料更新。

### 2.2 非目标

- 不更换 Steam 数据源，不抓取未获授权的第三方数据。
- 不把适配指数解释为购买概率、喜欢概率或跨请求可比较的绝对分数。
- 不在本轮执行生产数据库 retention、compact 或 `VACUUM`。
- 不在 `ora_proxy` 编译 Rust、Node 或 Docker 镜像；生产只拉取 GitHub Actions 产出的
  GHCR immutable 镜像。
- 不通过无限并发换取吞吐，也不牺牲 Steam 限流退避和 SQLite 单写者约束。

## 3. 现状证据

### 3.1 Worker 健康状态

- `deploy/mpgs-worker-loop.sh` 的 `--healthcheck` 对健康文件缺失、格式损坏、`status=error`、
  `running` 超过 `MPGS_WORKER_MAX_RUN_SECS`、`ok` 时间戳过期均返回普通非零值。
- `deploy/update.sh::deployment_healthcheck` 将上述所有非零值统一映射为返回码 3。
- 同 SHA 快速路径重试后遇到返回码 3 会直接成功退出并保持容器不动。

因此 #28 finding 成立。当前实现无法证明“超过 30 分钟仍在 running”是合法长任务还是
卡死任务；仅增加一个返回码仍不够，必须补充独立 heartbeat 和硬 watchdog 语义。

### 3.2 空字段退避

- `crates/storage/src/ingest.rs::ingest_store_details` 使用一个 `checked_empty` 表示名称、平台、
  语言或价格任一为空。
- `crates/storage/src/repo.rs::list_enrichment_targets` 同时用这个标志决定
  `needs_store_details` 和 `needs_price` 的退避截止时间。
- migration 25 只为 `store_detail_refresh_state` 增加了单一 `checked_empty`。

因此 #25 empty-backoff finding 成立。免费游戏会标准化为价格 0，不是主要问题；主要受影响
对象是未发售、区域暂不可购买或没有 `price_overview` 的条目。

### 3.3 一体化入库终态

- migration 25 将 `game_ingestion_queue.status` 限制为
  `pending/retry/complete`。
- `crates/storage/src/game_ingestion.rs::claim_tasks` 每次 claim 都增加
  `stage_attempts/total_attempts`。
- `retry_stage` 无最大尝试次数，只会再次写入 `retry`。
- Data Ops 快照只统计 pending、retry、leased 和四个阶段，没有 dead。

因此 poison item finding 成立。还需一并修正 attempt 语义：部署回收 lease 或进程退出不应
自动等价于一次业务失败。

### 3.4 推荐展示

- `apps/server/src/api.rs` 先生成推荐结果，再按可选的 CCU、评价数、发售日期或适配分进行
  标量重排；响应中的 `rank` 始终是最终响应顺序的一基位置。
- `web/src/screens/GameCard.tsx` 不知道当前排序方式，始终把 `rank` 传给 `ScoreBadge`。
- `web/src/app/format.ts::recommendationLabel` 对所有有效 rank 输出“第 N 推荐”。

因此在 CCU、评价数和发售日期排序中显示“第 N 推荐”是确定的语义错误。

### 3.5 当前吞吐边界

- Worker 默认每 60 秒运行一次，富化 limit 为 20。
- 一体化队列单次最多 claim 10 个 App，该限制目前是硬编码常量。
- 一体化新游按 `store_details -> review_summary -> popular_reviews -> ccu` 串行执行。
- 每次实际上游请求后默认等待 2500 ms，所有请求和所有 App 均串行。

直接提高 limit 只会拉长单次任务和健康时间窗。第一优先级应是跳过不需要的阶段，其次才是
有指标保护的并发与间隔调优。

## 4. 产品语义

### 4.1 生命周期采集档位

每个队列项必须持久化或可确定性推导 `enrichment_profile`：

| 档位 | 适用对象 | 采集范围 |
| --- | --- | --- |
| `basic_upcoming` | `upcoming/coming_soon` 的正式游戏 | 商店基础资料、发售状态/日期、类型/类别、平台/语言、封面、截图/视频、Demo 关系 |
| `basic_demo` | Demo/Playtest App 本身，且父游戏未发售或未知 | 与 `basic_upcoming` 相同，不请求评价、热门评价和 CCU |
| `full_released` | 已发售正式游戏，包括“已发售且提供试玩 Demo”的父游戏 | 基础资料、价格、评价汇总、热门评价、CCU 和完整推荐证据 |
| `full_override` | 人工明确关注且有业务理由的特殊条目 | 由管理员显式指定完整阶段，并保留审计记录 |

关键规则：

- “已发售游戏提供 Demo”时，完整采集对象是已发售父游戏；Demo 子 App 仍按基础档位处理。
- 日历页不建立独立重采集流程，直接读取 `basic_upcoming` 已维护的数据。
- 商店 `appdetails` 同一响应中顺带返回的价格可以落库，但价格缺失不能导致额外评价/CCU
  请求，也不能延长基础资料刷新周期。
- 任何条目从未发售变为已发售时，立即把 profile 升级为 `full_released`，从第一个尚未满足
  freshness 的完整阶段继续，不重复已经有效的商店请求。
- 已发售条目后来新增 Demo，只更新关系和可用性，不降低父游戏的完整采集档位。

### 4.2 基础资料完成定义

`basic_ready` 至少要求：

- AppID、规范名称、App 类型；
- `release_state`、原始发售文本、规范化日期及日期精度；
- 至少一个可用封面；
- 商店 genres/categories/tags 中可用于多人候选判断的数据；
- 平台和语言的“已知值”或“已检查为空”状态；
- 可获得时保存截图、影片/预告片和 Demo/父游戏关系。

评价、热门评价、CCU 和购买价格都不是未发售条目的 `basic_ready` 阻塞条件。

### 4.3 刷新优先级

建议默认优先级从高到低：

1. 刚从未发售转为已发售、尚未完成 full 的游戏。
2. 新发现且尚未 basic-ready 的条目。
3. 已发售推荐候选的过期评价、CCU 和价格。
4. 30 天内即将发售条目的商店基础资料。
5. 更远期或日期未定条目的基础资料。
6. 低价值媒体补全和英文名称补全。

建议 freshness：距发售 30 天内每天刷新一次商店基础资料，更远期每 7 天一次；发布状态
发生变化时不等待原周期。具体数值必须配置化，并由生产 429、吞吐和队列年龄指标校准。

## 5. 推荐名次与推荐指数

### 5.1 两者区别

推荐指数回答“在本次请求返回窗口中，这个游戏的相关性得分相对处于什么位置”。当前实现
使用 `relevance_score` 的上下文百分位，并且只有数据置信度和有效特征数量达标时才显示。
它不是概率，也不适合跨分区、跨用户、跨日期或跨分页直接比较。

推荐顺序与推荐指数排序使用同一 `relevance_score` 降序，分区规则、个性化和想玩信号在
生成该分数前生效；相同分数再使用稳定 tie-break。MMR 多样性和探索位不得改变公开头部
顺序，否则推荐名次会与推荐指数互相矛盾。

CCU、评价数和发售日期排序都是单字段标量排序。此时 API 的 `rank` 只是列表位置，不能称为
推荐名次。`fit_index` 作为兼容 API 名称继续表示推荐指数排序。

### 5.2 展示规则

- UI 使用“推荐指数”，API 保留兼容字段名 `recommendation_index` 和排序名 `fit_index`。
- `sort=recommended` 且全局 `rank <= 5`：显示“推荐第 N”及可用的适配档位/指数。
- `sort=fit_index` 且全局 `rank <= 10`：显示“推荐第 N”及可用的推荐指数。
- `sort=recommended` 且 `rank > 5`：不显示序数，只显示可用的适配档位/指数；资料不足时
  显示“资料较少，待观察”。
- `sort=ccu/reviews/release_date`：一律不显示“推荐第 N”，只显示当前排序值。
- 分页第二页不得重新产生“推荐前 5”；判断使用 API 的全局 rank，而不是页面内 index。
- 推荐 telemetry 仍保存完整 rank，不因展示隐藏而丢失归因数据。

### 5.3 前端实现约束

- `RankedFeedPanel` 必须把当前 `sort` 传给 `GameCard/ScoreBadge`。
- `ScoreBadge` 接收明确的 `showRecommendationRank`，不得自行猜测排序模式。
- 格式化函数分别生成“推荐名次”和“适配信息”，不再把两个概念硬拼成不可配置字符串。
- UI 测试覆盖推荐排序第 1、5、6 名，以及四种标量排序。

## 6. P0 可靠性修复

### 6.1 Worker 健康协议 v2

只靠一次任务开始时间无法区分合法长任务与卡死任务。健康协议应包含：

- `status`：`starting/running/ok/error`；
- `heartbeat_at`：由父循环独立周期更新，不依赖 dbtool 任务完成；
- `run_started_at`：本轮任务开始时间；
- `consecutive_failures`；
- 可选 `phase` 和 `child_pid`，用于诊断但不能单独作为健康证明。

行为要求：

- heartbeat 新鲜、子进程存在且运行时长未超过硬 watchdog：健康。
- 合法结构的健康文件只在短暂 heartbeat grace 内可返回专用“软过期”码。
- 文件缺失/损坏、未知 status、`status=error`、exec 失败、未来时间戳、子进程不存在、超过
  硬 watchdog：普通 hard-unhealthy。
- 硬 watchdog 必须实际终止失控子进程并让容器失败/重启，不能只改变 healthcheck 输出。
- `deployment_healthcheck` 只把专用软过期码映射为 3；其他 Worker 非零都返回普通 unhealthy。
- 同 SHA 快速路径可以短暂容忍软过期；重试后 hard-unhealthy 必须 controlled redeploy。
- 新 SHA 部署最终必须得到完全健康，软过期不能作为发布成功条件。

测试矩阵至少覆盖：缺文件、截断文件、非数字时间、`error`、新鲜 `ok`、过期 `ok`、新鲜
`running`、软 grace、超过 watchdog、exec 失败和 revision mismatch。

### 6.2 按维度 empty/backoff

用独立状态替代单一 `checked_empty`，最低要求包括：

- `store_core_empty`：名称/平台/语言等基础维度是否已检查为空；
- `price_empty`：价格是否已检查为空；
- `store_checked_at_ms` 与 `price_checked_at_ms`，或语义等价的独立 next-due 字段。

调度规则：

- `price_empty` 只影响 `needs_price`，不得影响 `needs_store_details` 和发售状态刷新。
- `store_core_empty` 不得把未发售条目的 release-state/date 刷新冻结 30 天。
- 免费游戏价格 0 是有效值，不标记为 `price_empty`。
- appdetails 请求仍可原子更新同一响应中的多个维度，但每个维度独立计算下次 due。
- migration 25 的旧 `checked_empty=1` 不能盲目回填为所有新维度都为空；首次调度应做一次有界
  重新确认，或根据已有 snapshot 推导。

### 6.3 一体化队列 dead 与 requeue

队列状态新增 `dead`，并新增或明确以下字段：

- `stage_failure_attempts`：只有确认的阶段失败才增加；
- `lease_count`：claim 次数，用于诊断，不消耗失败预算；
- `dead_at_ms`、`last_error_category`、有界错误摘要；
- `enrichment_profile` 和可选 profile 版本。

默认策略：

- `invalid_payload/parse_changed` 等确定性错误使用较小的 stage 上限。
- `network/rate_limited/storage` 使用更大的有限上限和封顶退避。
- 全局 auth/config 故障应暂停相关 lane 并告警，不能把整批 App 逐个打入 dead。
- deployment lease recovery 只释放 lease，不增加 `stage_failure_attempts`。
- 达到上限后进入 dead，不再占用普通 claim limit。
- Data Ops 显示 dead 总数、按 stage/category 聚合、最老 dead 时间和最近样例。
- 提供显式 CLI/admin requeue，要求填写原因并记录操作者、旧状态和时间；默认只能重置选定
  App/阶段，禁止无确认全量 requeue。

## 7. 数据获取加速方案

### 7.1 第一阶段：减少请求

- 调度器按 enrichment profile 跳过未发售/Demo 的三个非基础阶段。
- 已观察且 freshness 有效的阶段直接 advance，不 sleep、不发网络请求。
- store appdetails 同次响应完成基础资料、媒体、类别、发售状态和可选价格，禁止为这些字段
  重复请求。
- transition-to-released 进入最高优先级 full lane，避免被远期日历条目阻塞。

这一步应先上线。一个基础档位新游理论上从四类请求降为一类请求，收益远高于盲目扩大
并发。

### 7.2 第二阶段：解除硬编码瓶颈

- 将一体化 claim limit 10、请求间隔和各 lane 配额配置化，并提供安全上下界。
- 按 endpoint 建立独立 token bucket；429 只降低对应 endpoint 的速率。
- 允许很小的有界网络并发，初始上限建议 2；网络请求并发，但 SQLite 写入保持短事务串行。
- 并发结果按 App/阶段独立提交，单个慢请求不得阻塞整批结果落库。
- 取消最后一个请求后的固定尾部 sleep；限速在下一次请求 admission 前执行。
- Worker 单轮设置 wall-clock budget，接近预算时停止 claim 新任务并干净释放未开始 lease。

任何并发提升都必须通过生产指标逐级放量，不能直接把 `enrich_limit` 提到数百。

### 7.3 调度公平性

每轮为以下 lane 保留独立预算：

- `release_transition_full`
- `new_basic`
- `released_refresh`
- `upcoming_refresh`
- `media_optional`

任一 lane 连续 defer 超过配置上限后获得一个小的保底批次。派生 retrieval/quality 维护也要
保留 max-deferral，不能在 Steam 队列持续活跃时永久饥饿。

### 7.4 可观测性与目标

新增指标：

- queue depth、oldest age、claim/completion/dead，按 profile/stage/lane 分类；
- 每 endpoint 请求数、成功率、429、超时、P50/P95 latency；
- `basic_ready` 和 `full_ready` 的 P50/P95 time-to-ready；
- 每轮网络等待、SQLite 写入时间、`database is locked` 次数；
- 因生命周期策略跳过的请求数；
- Worker run duration、heartbeat age 和 watchdog termination。

首轮验收目标：

- 未发售/Demo 条目 0 次 review-summary、popular-review、CCU 请求，人工 override 除外。
- 新发现条目 `basic_ready` P95 小于 30 分钟。
- 转为 released 后 `full_ready` P95 小于 2 小时，且不会被远期 upcoming backlog 饿死。
- 在同等候选发现量下，单个未发售条目的上游请求数至少降低 60%。
- Steam 429 比例不高于优化前基线，SQLite lock 不增加。
- Worker 单轮不超过硬 watchdog；若超限必须终止并可恢复，而非无限 stale。

## 8. 数据模型与 API 影响

预计需要新 migration，禁止直接重写 migration 25：

- 扩展 `game_ingestion_queue.status` 到 `pending/retry/complete/dead`；SQLite 需要受控重建表并
  保留现有队列行。
- 增加 profile、失败尝试、lease 计数和 dead 审计字段。
- 拆分 `store_detail_refresh_state` 的空值/刷新状态。
- 数据状态快照和 Admin API 增加 dead/profile/lane 指标。

公共 feed API 保持兼容：

- 保留 `rank` 和 `recommendation_index`；前端根据响应 `sort` 决定展示。
- 后续 API 大版本可把 `recommendation_index` 更名为 `fit_index`，本轮不做破坏性字段删除。
- 不把未发售条目的缺失评价/CCU伪装成 0；继续使用 `null/unknown`。

## 9. 测试与验收矩阵

### 9.1 Storage/Worker

- 未发售游戏从 store_details 后直接达到 basic-ready，不执行后三个阶段。
- 已发售父游戏带 Demo 时父游戏走 full，Demo 子 App 走 basic。
- 发布状态切换会升级 profile 并只补缺失/过期阶段。
- price empty 不阻止次日 release-state/date 刷新。
- 免费价格 0 不触发 empty backoff。
- 每种错误分类在上限前 retry、到上限后 dead，dead 不再被普通 claim。
- lease recovery 不增加 failure attempts。
- 单 App requeue 可恢复，重复 requeue 幂等且有审计。
- migration 从 schema 25 保留 pending/retry/complete 任务及 stage。

### 9.2 Deployment

- Worker 健康协议 v2 的完整状态矩阵 shell 测试。
- 同 SHA + 软过期保持容器；同 SHA + error/坏文件/超 watchdog 触发 controlled redeploy。
- 新 SHA + 任何非完全健康状态不得发布成功。
- 部署恢复不会消耗一体化队列的真实失败预算。

### 9.3 Web/API

- 推荐排序只在 rank 1..5 显示推荐序号，rank 6 和第二页不显示。
- CCU、评价和发售日期排序不显示推荐序号；推荐指数排序只显示全局前 10 名。
- 适配信息在资料充足/不足时分别显示正确文案。
- 排序切换不会污染 cursor、telemetry rank 或返回位置恢复。
- 日历和 upcoming 对 review/CCU 缺失保持正常布局，不显示伪 0。

### 9.4 性能

- 固定 fixture 中比较优化前后每种生命周期的请求总数。
- 模拟高延迟、429、SQLite busy 和 poison item，验证 lane 公平性与 wall-clock budget。
- 使用冻结 fixture DB 运行 recommendation audit，确保采集字段变少不会破坏已发售推荐质量。

## 10. 实施顺序

### PR A：可靠性热修

- Worker 健康协议 v2、专用软状态和硬 watchdog。
- empty/backoff 分维度。
- ingestion dead、failure-attempt/lease-count 分离、Data Ops 统计与 requeue。
- 新 migration 和兼容性测试。

发布后观察至少 6 小时：同 SHA timer 不重复重建，hard-unhealthy 演练可恢复，队列无无限
retry 增长。

### PR B：生命周期采集与吞吐

- enrichment profile、发布状态升级和阶段跳过。
- lane 配额、max-deferral、可配置 limit/interval。
- 有界并发、endpoint 限速和新指标。

先以并发 1 部署验证“减少请求”的收益，再逐级放量到 2。每次调参至少观察一个完整高峰
周期。

### PR C：推荐展示语义

- 仅 Top 5 推荐名次。
- 标量排序隐藏推荐名次。
- “推荐指数”改为“适配指数”，拆分格式化与组件测试。

该 PR 不修改推荐算法和 telemetry，只修正展示语义，可独立回滚。

## 11. 生产发布与回滚

每个 PR 必须：

1. 在 GitHub Actions 完成 Rust tests、Web tests、fmt、clippy 和 immutable 镜像发布。
2. `ora_proxy` 只执行拉取与启动，不运行 build、Cargo、npm 或 Docker build。
3. 新 schema 首次部署前保留 1 份已验证数据库备份。
4. 验证 `/health/ready`、`/v1/meta`、Worker health、Data Ops 队列指标和关键 feed。
5. 新 migration 必须支持用上一 immutable 镜像加部署前备份恢复；不承诺旧二进制直接读取新
   schema。

任何 retention/compact 与功能发布分开执行，避免把数据清理故障和代码回归混在同一个
回滚判断中。

## 12. 生产空间治理

2026-08-10 已完成：

- `MPGS_BACKUP_RETENTION_COUNT=1` 已写入生产 `deploy/.env`。
- updater 使用内置保留逻辑删除 2 份旧完整备份，保留最新 1 份。
- 根盘使用率从 81% 降至 76%，可用空间从约 9.1 GB 增至约 12 GB。

后续候选：

| 对象 | 当前占用 | 处理建议 |
| --- | ---: | --- |
| systemd journal | 约 4.0 GB | 可先 vacuum 到 1 GB，并配置持久上限；保留最新日志，不直接清空 |
| 历史诊断库 `diag-20260803T110333Z.db` | 约 124 MB | 确认对应事故关闭且不再需要取证后删除 |
| 孤立 backup WAL/SHM | 约 64 KB | 基础 `.db` 已不存在，可删除，但收益可忽略 |
| APT cache | 约 60 KB | 无清理价值 |
| Docker 未使用对象 | 未得出可靠统计 | `docker system df`/全量 image 查询在生产超过 90 秒；不得盲目 prune |

禁止直接清理：当前数据库、正在使用的 Docker volume、当前/回滚所需镜像、未知归属文件、
未 dry-run 的数据表和仍在诊断窗口内的日志。

## 13. 完成定义

- 未发售和 Demo 基础档位不再请求无意义的评价/CCU，发布后自动升级完整采集。
- 新游基础资料和已发售完整资料达到本文 time-to-ready 目标。
- 推荐顺序突出前 5，推荐指数排序突出前 10；其他标量排序不伪装成推荐名次。
- 推荐顺序与推荐指数使用同一相关性次序，不再出现指数降低但名次仍被多样性槽抬高的矛盾。
- Worker 软过期与硬故障可区分，卡死不能被 updater 永久忽略。
- empty backoff 按维度生效，价格空值不再冻结 appdetails 更新。
- poison item 有 dead 终态、可观测、可审计 requeue，不再永久占用普通队列。
- 生产仍完全遵守 GitHub Actions/GHCR 构建、`ora_proxy` 只拉取运行的约束。
