# 推荐算法规格（rules-0.3）

## 0. 规格状态

本文定义 `rules-0.3` 的推荐行为和公开评分语义。状态按以下方式阅读：

| 能力 | 状态 |
| --- | --- |
| 模式规范化、置信度收缩的七维个性化、保守社区信号、结构化 MMR/探索 | `rules-0.3` 基线 |
| `rank` / `recommendation_index` / `data_confidence` / `friend_fit` / `slot_reason` 分离 | `rules-0.3` API 兼容升级；旧 `score` 暂时保留 |
| Feed/自然语言 run-item 归因、反馈及公开受控互动 event 写入与 90 天保留边界 | 服务端与 Web 主链路已接线；曝光仍以卡片挂载近似，Demo 无独立事件，写入失败不得计入归因指标 |
| 通用自然语言请求跨四分区召回、模式排除与最多 300 个候选 | `rules-0.3` 基线 |
| 活跃度 cohort 百分位、7/10 日覆盖门槛趋势、黄金矩阵离线评估器 | 计算与工具已接线；尚无已冻结的 200+ 人工标注矩阵或自动上线权重 |
| isotonic 行为校准、偏好学习、10%→50%→100% 放量 | 数据门槛后的后续阶段 |

`recommendation_index` 是当前请求实际返回页/窗口中的相对推荐指数，不是整分区/全目录百分位，也不是喜欢、购买、游玩或转化概率。未完成行为校准前，任何界面和文案都不得附加百分号或作概率解释。

## 1. 目标

推荐器优化的是“这个游戏是否适合当前熟人小组”，不是 Steam 总热度。它必须同时处理：

- 新游戏样本少但具有 Demo 或明确联机卖点。
- 热门老游戏口碑普通但仍有健康玩家生态。
- 经典合作/自建服游戏 CCU 较低但不依赖公共玩家。
- Steam 标签无法准确表达开房难度、自建服质量和主导玩法。

MVP 使用可解释规则和人工标注，不直接训练黑盒模型。

## 2. 输入与输出

输入：

- 游戏规范化特征和每项特征的证据可信度。
- 评论、CCU、价格、发布日期和服务状态快照。
- 用户偏好：人数、合作/竞技、时长、预算、平台、自建服意愿、语言。
- 分区和算法配置版本。

输出的核心语义：

```json
{
  "app_id": 548430,
  "section": "classic_legacy",
  "rank": 1,
  "recommendation_index": 94,
  "fit_band": "excellent",
  "data_confidence": 0.92,
  "friend_fit": 0.95,
  "slot_reason": "base",
  "score_calibration_version": "context-percentile-v1",
  "score": 0.91,
  "algorithm_version": "rules-0.3.0",
  "reasons": ["支持私人四人合作", "不依赖公共匹配"],
  "cautions": ["高难度任务需要稳定配合"],
  "components": {
    "friend_fit": 0.95,
    "group_fit": 0.96,
    "mode_fit": 0.90,
    "access_fit": 0.88,
    "hosting_fit": 0.93,
    "session_fit": 0.75,
    "quality": 0.94,
    "activity": 0.82,
    "freshness": 0.20,
    "risk": 0.04,
    "relevance_score": 0.91,
    "final_score": 0.91
  },
  "evidence_ids": ["feature:online_coop:548430", "review:548430:2026-07-13"]
}
```

- `rank`：多样性/探索重排后的完整最终顺序中的一基名次，不是当前页内序号。
- `recommendation_index`：当前请求实际返回页/窗口内，按连续相关性中位秩生成的 `0..100` 指数；证据不足时为 `null`。它不表示完整分区或全目录中的百分位。
- `data_confidence`：当前合并多人画像覆盖、评论、活动和日期置信度，用于逐信号先验收缩和是否展示指数；它不是正向排名奖励。
- `friend_fit`：熟人联机结构适配度，与资料置信度无关。
- `slot_reason`：`base`、`diversity` 或 `explore`，解释为什么最终名次可能不与指数严格单调。
- `score`：向后兼容的内部相关性分，不是概率；新客户端不得直接展示。
- `algorithm_version`：正在执行的代码/公式版本，`rules-0.3` 固定为 `rules-0.3.0`。
- Feed 根级 `config_version`：数据库当前激活的阈值和参数配置版本；升级旧数据库时可以暂时仍为 `rules-0.2.0`。它与代码 `algorithm_version` 不得合并或互相覆盖。

## 3. 多人联机分类

一个游戏可以拥有多个能力，但排序和过滤只使用统一的 `ModeFamily`。原始来源值先规范化，展示文案与排序枚举分离：

| 规范值 | 典型旧值/来源别名 |
| --- | --- |
| `private_coop` | `coop`、`co_op`、`online_coop` |
| `self_hosted` | `self_hosted_survival`、`dedicated_server`、`community_server` |
| `matchmade_pvp` | `competitive`、`versus`、`vs`、`pvp`、`pvp_only`、`matchmaking_competitive` |
| `public_world` | `public`、`shared_world`、`mmo`、`mmorpg` |
| `mixed` | `hybrid`、`coop_pvp`、`pve_pvp` |
| `generic_multiplayer` | `multiplayer`、`online_multiplayer` |
| `unknown` | 空值、无法可靠映射的值 |

主导模式只是能力模型的一部分：

| 维度 | 枚举示例 |
| --- | --- |
| 主导模式 | 上述 `ModeFamily` |
| 连接方式 | private_lobby, p2p, player_hosted, dedicated_official, dedicated_self_hosted, public_matchmaking |
| 服务器依赖 | none, optional, official_required, public_population_required, unknown |
| 加入方式 | invite, join_code, server_browser, direct_ip, matchmaking, unknown |
| 进度形态 | session, run_based, persistent_world, live_service, unknown |
| 人数 | min_players, recommended_min, recommended_max, hard_max |

Steam 的“多人”“在线合作”等标签只能形成来源证据，不能单独确定主导模式或自建服质量。`unknown` 不能当作正向证据，也不能仅因未知而视为明确不支持。排序内部使用 `ModeFamily`；公开 `multiplayer.dominant_mode` 为兼容旧客户端仍可能显示 `pvp` 等旧展示值，客户端不得反向用展示字符串实现过滤。

## 4. 证据与未知值

每个可争议特征保存：

```text
value + confidence + source_type + source_ref + observed_at
```

来源优先级默认如下：

1. 人工核验且附证据。
2. Steam 官方明确字段或开发者明确说明。
3. 两个以上独立来源一致推断。
4. 单一商店标签或 AI 文本推断。

缺失值不等于 `false`。数值型特征按以下方式收缩到同类先验：

```text
effective(x) = confidence * x + (1 - confidence) * cohort_prior(x)
```

同时单独计算证据充分度，防止“未知很多”的游戏获得虚假高分。

## 5. 候选硬过滤

目标上，硬过滤只表达客观不可玩或用户在本次请求中明确声明的限制；合作/竞技、自建服和公共匹配接受度属于软排序，不得在个性化生效前删除候选。

以下条件在评分前执行，社区信号和 AI 都无权恢复被淘汰候选：

- 不是可游玩的基础游戏、Demo 或明确关联的 Playtest。
- 已下架且无可购买/可运行路径。
- 已知多人服务关闭，且没有局域网、P2P 或自建服替代。
- 已确认不支持用户在本次请求中明确要求的平台。
- 已知推荐人数范围与用户的硬性人数完全不相交。
- 用户在本次请求中明确排除的内容、规范模式或同币种价格条件。
- 有可靠证据确认不具备多人能力。

平台、人数、价格、多人能力或服务状态未知时，候选保留并按置信度收缩；未知值不能作为通过硬条件的正向证明。

实现状态：`rules-0.3` 已取消 Recent/Popular/Classic 的个性化前 `friend_fit` 门槛，并通过请求级 `HardConstraints` 区分持久化软偏好与本次显式条件。普通 Feed 不再把长期人数、平台、语言、局时长和预算偏好自动当成硬过滤；相应查询参数与自然语言已应用条件才启用对应硬约束。已确认停服且无私人/自建替代路径仍是客观硬过滤。Steam Deck 仅识别独立的 `steamdeck_verified` / `steamdeck_playable` / `steamdeck_unsupported` 值；只有 OS 字段时保持未知，不能声称完成了 Deck 数据覆盖。

## 6. 核心特征

所有分项在进入公式前归一化为 `[0, 1]`。

本节同时记录 `rules-0.3` 基线与后续数据目标。当前分区公式实际消费 `F`、质量、活跃度/新鲜度、Demo/日期和风险的可用子集；下面标为“目标”的 90 日评论混合、cohort 百分位和事件校正不能被描述为已上线能力。

### 6.1 熟人联机适配度 F

```text
F_base = 0.22 * private_session
       + 0.20 * self_host_or_dedicated
       + 0.18 * online_coop
       + 0.15 * group_size_fit
       + 0.10 * low_public_population_dependency
       + 0.08 * drop_in_out
       + 0.07 * cross_platform_fit

F_penalty = 0.18 * matchmaking_core
          + 0.15 * public_world_dependency
          + 0.10 * group_size_mismatch
          + 0.08 * service_shutdown_risk
          + 0.06 * external_account_friction
          + 0.05 * platform_or_anticheat_restriction

F = clamp(F_base - F_penalty, 0, 1)
```

`self_host_or_dedicated` 必须区分官方专服与玩家可部署专服。只有后者能显著降低停服风险。

### 6.2 评价质量 Q

对正面数 `p`、总评价数 `n`，使用 Wilson 下界而不是裸好评率。`z=1.96`：

```text
phat = p / n
W = (phat + z^2/(2n) - z*sqrt(phat*(1-phat)/n + z^2/(4n^2)))
    / (1 + z^2/n)
```

```text
Q = 0.65 * Wilson(lifetime) + 0.35 * Wilson(recent_90d)
```

状态：`rules-0.3` 已优先使用可用的 lifetime Wilson，下界缺失时回退到裸好评率/中性先验；`recent_90d` 独立快照混合仍是数据阶段目标。

- 没有近期样本时，近期项回退到带低置信度的同类先验。
- 新游样本过少不会被判为差评，但会得到较低 `E`。
- 保存 Steam 官方过滤后的统计与原始统计；异常差异进入风险特征。
- 简中评价可作为“中文用户适配”辅助特征，不替代全语言质量。

### 6.3 证据充分度 E 与公开数据置信度

下面的 `E` 是后续多来源质量诊断目标，不是当前分区加分公式：

```text
review_volume = clamp(log(1 + total_reviews) / log(1 + 50000), 0, 1)
feature_coverage = weighted_known_features / weighted_required_features
source_quality = weighted_mean(source_confidence)

E = 0.45 * review_volume
  + 0.35 * feature_coverage
  + 0.20 * source_quality
```

`rules-0.3` 不把 `E` 作为分区正向奖励。公开 `data_confidence` 已按以下独立覆盖度合成，并只用于信号收缩、稳定同分决胜和指数展示门槛：

```text
profile_evidence = 0.60 * profile_source_confidence
                 + 0.40 * multiplayer_feature_coverage

data_confidence = 0.45 * profile_evidence
                + 0.25 * review_confidence
                + 0.20 * activity_confidence
                + 0.10 * release_date_confidence
```

评论置信度按评论量对数增长；活动置信度区分有效趋势、7 日典型 CCU、单点 CCU 和完全缺失。更细的多来源质量与逐特征 freshness 仍待接线。

当前实现不会因 Upcoming 缺少评论而重新归一化：`review_confidence=0` 会保守降低 `data_confidence`，但不会直接降低客观分区相关性。若后续改为分区内重新归一化，必须升级评分语义/配置版本并重新跑展示门禁。

### 6.4 活跃度 P

单次 CCU 波动大，使用 7 日窗口并在可比 cohort 内计算百分位：

```text
P = 0.60 * percentile(log1p(median_ccu_7d))
  + 0.25 * percentile(log1p(peak_ccu_7d))
  + 0.15 * normalized_trend(median_7d / median_28d)
```

状态：`rules-0.3` 已在完整查询候选集合内对 `typical_ccu_7d`（缺失时回退最新 CCU）计算同值共享 midrank 的 cohort 百分位；不经过候选列表的详情/搜索信号才回退到 CCU 对数归一化。7/28 日峰值比较仍是后续阶段。

当前 cohort 仅由本次分区查询集合界定；按联机依赖类型继续拆分是后续质量目标，以避免把四人合作游戏与 MMO 直接比较。CCU 只统计连接 Steam 的玩家，因此它是活跃度信号，不是真实总玩家数。

### 6.5 增长势头 M

```text
M = 0.45 * review_velocity_7d_vs_28d
  + 0.35 * ccu_trend_7d_vs_28d
  + 0.20 * update_or_release_event_freshness
```

状态：当最近 10 天至少有 7 个日聚合，并同时具备近期和基线窗口时，`rules-0.3` 才构造活动趋势；否则使用中性趋势，不再用 popularity 的倍数伪造 momentum。评论速度和事件校正尚未接入分区公式。

对发售首周、免费周末、大版本和促销事件做事件标记，避免把短期尖峰永久视为增长。

### 6.6 风险 R

风险是可叠加惩罚，主要包含：

- 多人服务器宣布关闭或持续不可达。
- 大规模异常评价波动。
- 发售日期反复变更或仍为模糊日期。
- 强制第三方账号、地区限制、反作弊导致的平台不兼容。
- 信息冲突或关键联机能力只有低置信 AI 推断。
- 价格/DLC 结构明显影响小组共同进入成本。

目标要求风险项生成可见提示，不能只暗中扣分。当前 `rules-0.3` 已把已知服务关闭/公共依赖接入风险信号，但解释器尚未保证每个风险分量都有对应 caution；该覆盖率必须由后续解释门禁约束。

## 7. 分区规则

四分区先计算不含玩家画像的客观分区相关性 `B_section`，再在第 8 节统一执行 `relevance = 0.55 * B_section + 0.45 * U`。`friend_fit` 是独立展示/解释字段和精确同分后的稳定决胜项，不在 `B_section` 中重复加权。每个分区的风险扣分均为 `0.20 * R`。

### 7.1 最近发售

候选门槛：

- 正式发售 `0～180` 天。
- 无多人服务关闭等硬风险。

```text
B_recent = 0.40Q + 0.20P + 0.15M + 0.25*freshness - 0.20R
```

评论不足时允许出现，并减少公开指数判断使用的有效特征数；资料不足时显示“待观察”。当前 `data_confidence` 合并多人画像、评论、活动和日期覆盖度。置信度用于先验收缩、稳定同分决胜和展示判断，不作为“资料越全分越高”的线性奖励项。

### 7.2 即将发售/Demo

候选门槛：

- 已知发售日在未来 `30` 天内；或当前存在可玩的 Demo/公开 Playtest。
- 有至少一项多人能力证据。
- 超过 30 天、仅有模糊年份/季度或 TBA 的本体不进入 Upcoming；可玩的 Demo/Playtest 不受本体 30 天窗口限制，但必须明确标识。

当前实现把实际 `app_type=demo|playtest` 的 App 自身作为候选，不要求其父游戏也在 30 天窗口；本体关系校验和分组展示仍是后续数据质量项。

```text
B_upcoming = 0.35*demo_playability
           + 0.20*release_date_confidence
           + 0.25*release_proximity
           + 0.20*studio_prior - 0.20R
```

`studio_prior` 权重受限，防止大厂天然压制独立游戏。

### 7.3 人气老游

候选门槛初始值：

- 发售超过 180 天。
- `typical_ccu_7d`（缺失时最新 CCU）不低于活动配置的 `popular_min_ccu`，默认 `1000`；cohort 百分位当前参与评分，不替代该资格门槛。
- Wilson 质量下界通常不低于 `0.58`；CCU 达到配置的 `popular_high_ccu`（默认 `100000`）时可放宽到 `0.55`，不得完全取消。

```text
B_popular = 0.40P + 0.20M + 0.30Q
          + 0.10*maintenance_health - 0.20R
```

这实现“人气可部分豁免好评度加权”，但不会把持续差评的游戏无条件推荐。

### 7.4 经典老游

候选门槛初始值：

- 发售超过 180 天。
- 总评价数不少于 `3000`。
- Wilson 质量下界不低于 `0.82`。
- 若依赖公共匹配，必须满足最低活跃度；自建服、私人房或 P2P 可豁免。
- 排除已经进入 Popular 的候选，保持四分区互补。

```text
B_classic = 0.45Q + 0.25*longevity
          + 0.15*maintenance_health + 0.15P - 0.20R
```

`rules-0.3` 不再用个性化前的 `friend_fit` 门槛过滤 Recent、Popular 或 Classic，因此竞技偏好玩家仍可看到已确认可玩的匹配型游戏。质量、活跃度与时间门槛仍是种子参数，必须通过真实数据分布校准，不能长期作为不可变业务常量。

服务从活动 `algorithm_configs.config_json` 读取并校验这些门槛；配置内容和版本共同进入游标/ETag 上下文。配置变化后旧游标失效，避免跨规则版本续页。游玩意愿 revision 同样进入游标，投票改变排序后客户端必须从第一页重新分页。

## 8. 个性化

个人/小组适配度 `U` 拆成彼此独立的分量，避免多个合作能力简单相加后饱和：

```text
fit_i = confidence_i * observed_fit_i + (1 - confidence_i) * 0.5

U = 0.25 * group_fit
  + 0.20 * mode_fit
  + 0.20 * access_fit
  + 0.15 * hosting_fit
  + 0.10 * session_fit
  + 0.07 * budget_fit
  + 0.03 * language_fit
```

```text
personalized_relevance = 0.55 * section_relevance + 0.45 * U
```

明确的用户硬条件在候选过滤阶段执行；`U` 只处理软偏好。候选缺少某一维证据时，该维回到 `0.5`，不能因“未知较多”获得满分，也不能把平台支持误写为真实跨平台联机能力。

Migration `0019_preference_confidence` 为持久化偏好增加整体置信度：升级前已有行保留 `1.0`，新建行显式写 `0.0`。未确认默认值会把七个个人分量收缩到 `0.5`；请求级显式条件只把对应维度立即视为高置信。该字段不会自动从一次点击推断，客户端确认设置时必须明确提交。

已知的平台、语言、典型局时长、人数和同币种价格仅在本次请求显式启用相应约束时硬过滤；字段未知时保持候选资格并以中性值参与个性化，遵守“缺失不等于 false”。

当前反馈行为：

- “不感兴趣”隐藏该 AppID；撤销后退出当前反馈上下文。
- 活动状态按同一 AppID 的 sentiment、ownership 和各 reason 分组保留，因而“玩过”不会覆盖喜欢/不喜欢，多个原因也可以共存；现有 HTTP 写入仍使用单个 legacy `type`。
- “喜欢”“玩过”“太竞技”“人数不合适”“开服麻烦”目前只对同一 AppID 叠加固定调整或隐藏，不跨游戏学习类型偏好。
- 负反馈保留原因枚举，文本说明可选且默认不发送 AI。

把反馈拆成 sentiment、reason tags、ownership/played 和 play intent，并用 90 天半衰期更新可撤销的跨游戏偏好后验，属于获得可靠 run 归因后的阶段；手工偏好必须始终优先。

### 8.1 游玩意愿票（社区信号，rules-0.3）

`play_intent_votes` 聚合出每个游戏的社区「想玩」票数，是跨用户信号，与个人反馈分开：

```text
excess = max(distinct_voters - 5, 0)
boost = min(configured_weight, 0.03) * excess / (excess + 20)
relevance_score = relevance_score + boost
```

- `0～5` 个不同账户不改变排序；第 6 票起才形成保守加成。
- 加成发生在确定性打分、个性化与同游戏反馈之后、MMR 之前；单调且最多增加 `0.03`。
- 票数不能恢复被硬过滤或明确负反馈淘汰的候选。
- `play_intent_weight <= 0` 或 `play_intent_saturation = 0` 表示禁用。旧配置即使保存了更高权重，运行时仍按 `0.03` 上限裁剪。
- 这是全站社区信号，不代表好友或当前小组意愿；客户端必须使用“全站玩家想玩”等准确文案。

### 8.2 公开推荐指数

内部使用连续的 `relevance_score` 排序，风险扣分后的负值也保留，避免在中间阶段裁剪为大量 `0`。向后兼容的 `final_score` 可以限制到 `[0,1]`，但不得用作玩家可见概率。

先确定最终排序和本次请求实际返回窗口，再只用该窗口内条目的 `relevance_score` 降序计算中位秩。Feed 的窗口等价于排序后 `skip(offset).take(limit)` 的实际结果，`N` 是实际返回条目数：

```text
recommendation_index = round(100 * (N - midrank + 0.5) / N)
```

- 精确同分共享同一 `midrank` 和指数，不用 AppID 制造假差异。
- 返回窗口少于 `10`、`data_confidence < 0.45` 或有效独立特征少于 `3` 时返回 `null`，客户端显示“资料较少，待观察”。
- `fit_band`：指数 `80..100` 为 `excellent`，`60..79` 为 `good`，其余非空指数为 `consider`，无指数为 `insufficient_data`。
- 最终顺序决定窗口成员，但窗口内的指数只取决于连续相关性的中位秩；完整最终顺序中的 `rank` 单独保留。因此 MMR 仍可能让指数较低的多样性/探索条目排在指数较高的条目前，此时必须通过 `slot_reason` 明示原因。
- 改变分页、`offset` 或 `limit` 会改变比较窗口，因此同一游戏的指数可能变化；精确同分条目在同一窗口内共享指数，不用 AppID 制造假差异。

真实只读快照的改造前重放中，Classic 即使按完整分区池计算也只有 8 个不同指数，头部仍被大池中位秩压缩。返回窗口语义让玩家当前看到的头部条目获得更可辨的相对刻度，但不会拆开 `relevance_score` 精确同分，也不能替代特征区分度门禁。

Feed 的 `sort=recommended` 保留上述多样性/探索编排并忽略方向；`sort=fit_index`（输入别名 `relevance` / `fit`）则严格按连续 `relevance_score` 排序并支持 `asc` / `desc`。`fit_index` 是排序模式名，不改变公开指数的上下文百分位语义，也不把它变成概率。

## 9. 多样性与探索

基础排序按以下确定性决胜顺序产生：

```text
relevance_score desc
-> data_confidence desc
-> quality desc
-> freshness/date proxy desc
-> stable user-day hash asc
-> app_id asc
```

用户每日哈希由请求内账号身份（未登录时为 `public`）与 UTC 日期在内存中派生，不返回、不记录原始身份或 seed；日期进入游标/ETag 上下文，跨日不会复用旧排序。无请求身份的内部兼容入口继续使用 AppID 顺序。AppID 仅是哈希碰撞后的技术兜底；公开指数仍让精确 `relevance_score` 同分共享同一值，不制造假精度。

重复 AppID 只保留相关性最高的一项。前 200 个候选再使用 MMR，尾部保持基础顺序：

```text
MMR(candidate) = 0.85 * relevance
               - 0.15 * max_similarity(candidate, selected)
```

相似度只在双方都已知的结构化维度上计算，然后按已知权重重新归一化：

```text
0.45 * taxonomy_tag_jaccard
+ 0.25 * mode_capability_jaccard
+ 0.15 * publisher_equality
+ 0.15 * series_equality
```

模式相似度先把规范 `ModeFamily` 映射为合作、私人房、自建、匹配、PvP、公共世界能力位，再计算 Jaccard；`generic_multiplayer` 只与自身相似，无法解析的模式视为未知。未知维度不贡献相似或不相似；`friend_fit` 接近程度和解释文本不得参与相似度或探索判断。

当前候选已接入 `catalog_taxonomy` 中的 categories/genres 标签和第一个 publisher；series 字段已预留但目录尚未物化，因此当前系列维度通常不参与相似度。

探索约束：

- 首页最多 2 个探索位，只能出现在最终第 `5～8` 位。
- 候选的基础名次必须在第 13 名或以后，相关性不得低于 Top12 截止值的 10% 相对边界。
- `data_confidence >= 0.45`，至少有一项结构化多样性元数据，且相对已选集合的 novelty 至少 `0.25`。
- 没有合格候选时不强制填充探索位。
- 每个条目输出 `slot_reason=base|diversity|explore`；界面必须标识多样性/探索带来的名次变化。

当重排池至少包含三种已知模式且存在可行解时，当前 MMR 已对 Top20 启用单一模式最多 `60%` 的守门；稀疏或不可满足的池不会强塞或丢弃候选。严格的系列/发行商数量上限仍只是后续质量目标。

## 10. AI 二次排序

AI 仅接收确定性候选与证据摘要。有效 AI 分数为：

```text
ai_effective = ai_confidence * ai_fit
             + (1 - ai_confidence) * base_score

blended_score = 0.85 * base_score + 0.15 * ai_effective
```

规则：

- `base_score` 优先取进入 AI 阶段时完整的 `relevance_score`，不能退回到丢失社区/反馈影响的早期分项；只为旧响应兼容回退到 `score`。
- AI 权重最大 `0.15`，不能改变分区、候选成员或硬过滤结果。
- AI 返回 AppID 必须属于本次候选集。
- AI 返回数组顺序没有排序权限；服务端只应用数值调整，再按混合分、原确定性位置、AppID 稳定排序。
- 每条理由必须引用已提供 `evidence_id`。
- 无效、超时或低置信输出回退到未调整的确定性成员和顺序。
- AI 可以指出新的待审核特征，但不能立即把它提升为高置信事实。

## 11. 推荐解释

解释由确定性模板优先生成，AI 可做语言润色但不能改变事实。

解释完整性目标为每个条目至少包含：

- 两个最强正向原因。
- 最重要的一个风险或限制；没有已知风险时显示“暂无已确认限制”，不能声称没有限制。
- 适合人数、联机方式和公共玩家依赖。
- 数据快照时间与主要证据来源。
- “为什么出现在该分区”的简短说明。

当前 Feed 返回 `reasons`、`cautions`、兼容 `evidence_ids`、卡片级 `reason_evidence[]`，以及 `multiplayer/reviews/activity/price/release` 五组 `feature_freshness`。它仍未提供逐句理由到证据的一一映射，也未对每张卡建立完整的“两条差异理由 + 一条风险/未知”门禁；在该门禁完成前，不能把完整解释目标描述为已验收能力。

## 12. 计算策略

```text
标准 Feed：候选召回 -> 客观硬过滤 -> 特征构造
          -> 分区/个性化/反馈/社区分 -> 稳定相关性排序
          -> MMR/探索 -> 最终 rank

目标自然语言链路：结构化意图 -> 跨分区混合召回 -> 客观硬过滤
                -> 基础/个性化/反馈/社区/检索分
                -> 可选 AI 数值融合 -> 稳定排序 -> MMR/探索
                -> 最终 rank
```

这是 canonical ordering：任何后处理都不能恢复硬过滤候选，AI 必须在数值融合后按分重排，最终多样性阶段必须保留 `slot_reason`。

`rules-0.3` 已实现目标自然语言顺序：先对已同步目录执行 FTS + 向量 `RRF(k=60)`，得到最多 300 个去重 AppID；再从每个适用分区的完整已排名集合中准入仍通过分区规则、显式硬条件和负反馈的命中，剩余容量按严格相关性补齐，最终联合池最多 300。AI 只做有界数值融合，之后执行一次全局 MMR 并刷新 rank/指数。内部四个分区 Feed 不写 run，最终自然语言响应只写一个归因 run。检索为空或失败时保持确定性回退。

不在每个普通推荐请求中调用 AI。相同输入按以下缓存键复用：

```text
algorithm_version + feature_snapshot + preference_hash
+ normalized_query_hash + ai_model + prompt_version
```

## 13. 测试与评估

### 13.1 黄金测试集

阶段门禁要求至少 200 条“玩家画像 × 真实游戏”的 `0..3` 相关性标注，覆盖：

- 私人合作：深岩银河、雨中冒险 2 等。
- 自建服生存：方舟、帕鲁等。
- 匹配核心竞技：CS2、永劫无间等。
- MMO/公共世界、派对游戏、停服游戏、错误人数和 Demo 关系。

核心断言：

- 默认偏好下，私人合作/自建服组整体高于公共匹配核心组。
- 自建服经典游戏不会仅因低 CCU 被淘汰。
- 人气老游可以降低质量权重，但差评底线仍生效。
- 改变人数、竞技偏好和自建服意愿后，排序方向符合预期。
- AI 关闭、超时和输出攻击场景与确定性结果可用性一致。

仓库已提供只读离线评估器，但没有内置或伪造生产黄金标签：

```powershell
mpgs-dbtool recommendation-golden-evaluate .\labels.json --json
```

输入必须使用 `schema_version="recommendation_golden_labels_v1"`，至少包含 200 条唯一的 `persona_id + app_id + section` 判断、至少 5 个 persona 和 5 个游戏。`relevance` 为 `0..3`；`personal_fit`、质量/活动/趋势/新鲜度/Demo/日期/工作室/长青度/维护/风险，以及 `ccu_baseline`、`review_baseline` 均须显式提供有限的 `0..1` 值，未知值不能静默当成零。示例行：

```json
{
  "schema_version": "recommendation_golden_labels_v1",
  "labels": [{
    "persona_id": "four-player-private-coop",
    "app_id": 548430,
    "section": "classic_legacy",
    "relevance": 3,
    "personal_fit": 0.95,
    "quality": 0.88,
    "activity": 0.76,
    "momentum": 0.52,
    "freshness": 0.10,
    "demo": 0.0,
    "date_confidence": 1.0,
    "studio_prior": 0.60,
    "longevity": 0.82,
    "maintenance": 0.90,
    "risk": 0.05,
    "ccu_baseline": 0.76,
    "review_baseline": 0.88
  }]
}
```

工具执行分区级 pairwise logistic、L2 正则、非风险权重非负且归一化、风险权重非正、个人权重限制 `0.35..0.55`，并分别执行 persona/game 五折留出。`freeze_eligible=true` 只有在四分区均可训练、每个留出策略的所有折可评估、聚合 `NDCG@20 >= 0.80`、成对方向准确率 `>= 0.90`，且相对当前规则、CCU、评论三个基线都至少提升 `5%` 时出现。命令只输出候选权重和报告，不写配置或数据库；`freeze_eligible=false` 本身不会使命令非零退出，自动化必须解析 JSON。矩阵未由真实人工标签冻结前，禁止声明权重经过玩家行为校准。

### 13.2 离线指标

- `Precision@20`：首屏熟人联机合格率。
- `NDCG@20`：人工相关性排序质量。
- 分区准确率和跨分区重复率。
- 类型、系列和发行商覆盖率。
- 低置信候选曝光占比。
- 解释证据覆盖率。

进入 `rules-0.3` 全量默认前的门槛：

- `NDCG@20 >= 0.80`，且相对当前规则与 CCU/评论排序基线至少提升 `5%`。
- 硬约束违规率为 `0`，关键成对方向正确率至少 `90%`。
- 对有效特征向量不同的 Top20，至少有 12 个不同展示指数，最大分桶不超过 `20%`，最大精确同分组不超过 2。
- 内部相关性边界裁剪率不超过 `1%`；若证据向量相同则允许同分，但必须显示资料不足。
- MMR 相对严格相关性排序的 `NDCG@20` 损失不超过 `3%`。
- AI 候选泄漏和硬过滤恢复均为 `0`；AI 单条数值影响不超过 `0.15`。

对真实数据库的确定性门禁使用只读审计：

```powershell
mpgs-dbtool recommendation-audit <db> --as-of 2026-08-02 --top 20 --json --strict
```

该命令以 SQLite `read_only/query_only` 打开数据库，不迁移、不写入。`--user-id` 可选择现有画像；省略时使用默认画像。`--top 1..100` 只控制展示摘要，质量门禁固定检查 Top20，不能用较小 `--top` 绕过。`--strict` 在任何适用的确定性门禁失败时返回非零：裁剪率 `<=1%`、满足可评估条件时 Top20 至少 12 个指数且最大指数桶 `<=20%`、跨不同有效证据向量的精确同分组最多 2、可行三模式池的单模式占比 `<=60%`、超过 3 个指数点的 MMR 倒置均有 slot reason，以及解释证据 ID 100% 可解析。`not_applicable` 不计失败，但也不代表通过。该审计没有人工标签或线上结果时不计算 NDCG、成对相关性或校准质量。

### 13.3 在线指标

- 详情打开、Steam/Demo 跳转和明确负反馈。
- “人数不合适”“太竞技”“开服麻烦”的原因率。
- AI 回退率、无效输出拦截率和单次成本。
- 不优化单纯停留时长，避免制造无效浏览。

只有每分区至少获得 300 个可归因的正负结果后，才允许拟合按曝光位置校正的 isotonic calibration；门槛之前继续使用 `context_percentile_v1`。上线顺序为影子对比，再 `10% -> 50% -> 100%` 放量，并以隐藏率、无结果率和硬约束违规率为回滚护栏。

### 13.4 第二、三阶段实现边界

| 项目 | 当前状态 |
| --- | --- |
| 七维个性化、逐多人能力置信度、真实 Wilson/活动百分位/日期与风险信号 | 已接线；局时长、跨平台、语言等来源覆盖仍不足，不能把未知当成已支持 |
| 结构化 MMR、探索窗口、Top20 60% 模式守门 | 已实现；series 通常未物化，发行商/系列更细的数量上限未实现 |
| FTS + 向量 RRF、四分区完整排名后准入、300 联合池、AI 后一次全局 MMR | 已实现；只覆盖已同步的本地目录，索引未完成时确定性回退 |
| 200+ 黄金矩阵训练与双五折评估 | 评估器已实现；仓库没有可宣称生产有效的人工矩阵，权重不会自动写入配置，也不直接验收 MMR 相对相关性排序的 NDCG 损失 |
| 解释与 freshness | 已返回卡片级 `reason_evidence[]` 和五组 `feature_freshness`；仍缺逐句证据映射与逐卡解释完整性门禁 |
| 反馈正交状态 | Storage 读取已按 sentiment/ownership/reason 分组；公开 HTTP 仍是一次一个 legacy `type`，UI 原因收集和完整正交 wire shape 未完成 |
| run/item/event 归因 | run/item、反馈 event、公开 `exposure/detail_open/steam_click/play_intent` 写入接口、Web run 透传/四类上报和 90 天清理已接线；曝光仍以卡片挂载代理而非 viewport 确认，Demo 跳转没有独立枚举，`subject_hash` 也尚未配置部署密钥 HMAC |
| 跨游戏偏好学习、90 天半衰期后验 | 未实现 |
| 每分区 300 结果门槛、位置偏差校正、isotonic calibration | 未实现；`score_calibration_version=context-percentile-v1` 只是当前百分位语义版本 |
| 影子运行与 `10% -> 50% -> 100%` 放量/即时回滚 | 未实现自动化控制面；活动配置可版本化不等于已具备实验放量系统 |

### 13.5 候选发现与「新游进池」（算法优化 backlog）

**问题（已线上踩坑）**：联机候选来自 Steam 商店搜索 `category2=1`。若排序为 `Reviews_DESC`，则「刚发售、评价很少/为零」的典型派对/联机新作（例如 2026-07-30《机械狂欢》app 4108000）几乎不会出现在前几页，导致 **发现阶段漏检**，而不是 Feed 排序把它藏起来。

**已落地基线（`store-search-0.2` / 生产补丁）**：

- 默认排序改为 `Released_DESC`（按发售日新→旧）。
- 游标键改为 `steam_store_search:multiplayer:released_desc`，避免沿用旧 Reviews 偏移。
- 调度在候选数已达目标时仍刷新首页，以捕获最新发售。

**后续优化（未做，记入算法迭代）**：

1. **双通道合并**：`Released_DESC`（新游）+ `Reviews_DESC`（口碑）分页并集，按 app_id 去重，防止只追新或只追热。
2. **地区/语言**：`cc`/`l` 与运营区一致（如 `cn`/`schinese`），减少英文区列表偏差。
3. **新游保送配额**：`recent_release` / 日历窗口内，对「发售 ≤14 天且联机证据成立」保留最低曝光比例，避免仅靠推荐分被老盘压没。
4. **日历与发现对齐**：即将发售有日期候选过少时，监测页应提示「发现偏旧」而非仅显示红条。
5. **强制入库运维路径**：对运营点名的 app_id（如 4108000）提供一键 store enrich，不依赖搜索页运气。

改动发现排序或配额时必须更新 `adapter_version` / `config_version`，并在黄金集或抽检集中验证「新发售联机作可被发现」。

## 14. 版本管理

每次推荐保存：

```text
algorithm_version
config_version
feature_snapshot_at
candidate_set_hash
ai_model (optional)
prompt_version (optional)
```

修改权重、阈值、缺失值策略或分区规则都必须生成新 `config_version`。发布前必须对旧版本和新版本运行同一黄金集并保存差异报告。

API 必须分别返回代码 `algorithm_version` 与活动 `config_version`。旧数据库继续使用经校验的旧配置是兼容行为，不表示服务仍在执行旧算法；迁移或切换活动配置只改变 `config_version`，代码部署才改变 `algorithm_version`。

`recommendation_runs/items/events` 默认保留 90 天，只保存结构化上下文/候选集哈希和受控事件元数据，不保存自然语言原文。Feed 与自然语言响应已为成功持久化的最终结果返回独立 run ID，反馈会验证 `(run_id, app_id)` 并写入归因 event；只有实际带非空 run ID 且归因率达到 `99.5%` 后，在线指标才能作为校准或放量依据。
