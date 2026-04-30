# AI 分析功能设计 Spec

- 文档日期：2026-04-30
- 项目目录：`D:/AI Coding/mpgs`
- 适用范围：单游戏 AI 分析第一阶段，以及后续 AI 推荐助手对分析结果的复用
- 文档目标：把当前占位型 AI 评估升级为可缓存、可解释、可降级的证据型分析系统

## 1. 设计目标

本阶段的目标不是直接做完整“聊天推荐助手”，而是先把单游戏分析引擎做扎实，形成一套可复用的分析结果生产能力。

第一阶段需要解决的问题：

- 当前详情页 AI 区只有短评和模拟分数条，缺少完整分析结构
- 当前后端 AI 返回结构过薄，只适合“一次性短评”，不适合详情页深度展示
- 当前系统没有“最近一次完整分析报告”的缓存，导致每次展示都可能退化成临时结果
- 后续 AI 推荐助手如果没有统一的单游戏分析结果，就会重复生成、重复判断、重复解释

第一阶段完成后，系统应具备以下能力：

- 进入游戏详情页后，可以自动得到一份完整的证据型分析报告
- 分析报告包含清晰结论、维度评分、亮点、风险、结构化证据、评论摘录证据
- 报告会缓存最近一次结果，并允许用户手动刷新
- LLM 不可用或失败时，系统仍能稳定展示规则版报告
- 推荐助手第二阶段可以直接复用这份报告，而不是重新发明单游戏分析

## 2. 已确认的产品决策

本设计基于以下已确认决策：

1. 开发顺序：先做单游戏分析引擎，再复用到 AI 推荐助手
2. 报告类型：证据型报告，而不是纯结论卡片或购买顾问
3. 生成方式：混合型
   - 规则层负责维度判断、证据整理、评论摘录选择
   - LLM 负责生成更自然的总结文案
   - LLM 失败时降级为规则版报告
4. 展示位置：详情页内展示“摘要 + 可展开完整报告”
5. 证据形式：结构化证据 + 评论摘录
6. 缓存策略：每个游戏只保存最近一次完整分析报告
7. 生成时机：首次进入详情页若无缓存则自动生成；已有缓存则直接展示；用户可手动刷新

## 3. 范围边界

### 3.1 本阶段范围内

- 详情页 AI 标签页升级
- 新的完整分析报告数据结构
- 最近一次分析报告的本地缓存
- 自动生成与手动刷新
- 混合分析与规则降级
- 前后端类型、命令、数据库字段与测试补齐

### 3.2 明确不在本阶段范围内

- 分析历史
- 批量预生成所有游戏的完整分析
- 独立 AI 分析页
- 跨游戏对比报告
- 购买时机、折扣判断、价格趋势分析
- 完整自然语言推荐助手
- 复杂提示词管理后台

## 4. 当前基线

当前代码已经具备一条可用但很薄的 AI 评估链路：

- 前端详情页已有 `AI 评估` 标签和 `重新 AI 评估` 按钮
- 独立的 `AI 智能推荐助手` 页面已存在占位 UI
- 后端已有 `assess_game_with_ai(appid)` 命令
- `games` 表已有 `ai_score`、`ai_summary` 字段

当前问题在于：

- `ai_summary` 只适合列表短评，不适合完整报告
- 当前详情页里的分数条是静态衍生展示，不是真正的分析结果
- 后端没有“最近一次完整分析报告”的结构与持久化
- 前端状态仍偏向“全局单次 assessment”，不适合详情页按 `appid` 管理分析结果

因此，本阶段需要把“轻量 AI 字段”和“完整 AI 报告”分层。

## 5. 核心架构

### 5.1 轻量字段与完整报告分层

系统保留现有轻量字段：

- `aiSummary`
- `aiScore`

这些字段继续服务于：

- 首页卡片
- 列表摘要
- 详情页侧栏简述
- 第二阶段推荐结果卡片

系统新增独立的完整报告对象：

- `GameAnalysisReport`

这个对象只负责：

- 详情页 AI 标签页的完整证据型展示
- 第二阶段推荐助手的深度解释复用

设计原则：

- `GameCard` 是轻量浏览对象
- `GameAnalysisReport` 是深度分析对象
- 列表展示不依赖完整报告
- 完整报告生成成功后，可以同步反哺轻量字段

### 5.2 生产者与消费者

第一阶段把单游戏分析定义为统一的“生产者”：

- 输入：单个 `GameCard` 的本地元数据
- 输出：`GameAnalysisReport`

第二阶段推荐助手只作为“消费者”：

- 优先消费已有 `GameAnalysisReport`
- 只补做“需求匹配、排序、推荐理由组合”
- 不重复设计单游戏分析逻辑

## 6. 完整报告数据结构

`GameAnalysisReport` 采用“面向展示”的结构，不直接暴露模型原始响应。

建议结构如下：

```ts
type AnalysisSource = "hybrid" | "rule";
type AnalysisConfidence = "high" | "medium" | "low";
type AnalysisEvidenceKind =
  | "positive_review_pct"
  | "total_reviews"
  | "current_players"
  | "tags"
  | "multiplayer_modes"
  | "short_description"
  | "review_snippet";
type AnalysisReviewStance = "strength" | "risk";

interface GameAnalysisReport {
  appid: number;
  generatedAt: string;
  source: AnalysisSource;
  confidence: AnalysisConfidence;
  overallScore: number;
  overview: string;
  dimensionScores: AnalysisDimensionScore[];
  strengths: AnalysisPoint[];
  risks: AnalysisPoint[];
  evidence: AnalysisEvidenceItem[];
  reviewEvidence: AnalysisReviewEvidenceItem[];
}

interface AnalysisDimensionScore {
  key:
    | "approachability"
    | "multiplayer_fun"
    | "content_depth"
    | "reputation_stability"
    | "activity_health";
  label: string;
  score: number;
  reason: string;
}

interface AnalysisPoint {
  title: string;
  reason: string;
}

interface AnalysisEvidenceItem {
  kind: AnalysisEvidenceKind;
  label: string;
  value: string;
  interpretation: string;
}

interface AnalysisReviewEvidenceItem {
  stance: AnalysisReviewStance;
  quote: string;
  playtimeText: string;
  interpretation: string;
}
```

### 6.1 字段语义

- `generatedAt`
  - 最近一次报告生成时间
- `source`
  - `hybrid`：规则层 + LLM 总结成功
  - `rule`：LLM 失败后由规则层独立产出
- `confidence`
  - `high`：关键结构化数据齐全，且评论摘录可支撑主要判断
  - `medium`：核心结构化数据存在，但评论样本或活跃度数据不完整
  - `low`：仅能依据少量结构化数据给出基础判断
- `overallScore`
  - 本次完整分析的综合分，用于回写 `aiScore`
- `overview`
  - 面向用户的 1 到 2 句总评，回答“整体是否值得进一步关注”

### 6.2 固定维度

第一阶段维度固定为 5 个，不允许模型自由发明：

1. `approachability`
   - 上手门槛
2. `multiplayer_fun`
   - 多人乐趣
3. `content_depth`
   - 内容耐玩度
4. `reputation_stability`
   - 口碑稳定性
5. `activity_health`
   - 活跃度健康度

这样做的目的是保证：

- 前端布局稳定
- 规则层可控
- 推荐助手第二阶段可直接复用维度结果

### 6.3 证据条目

第一阶段 `evidence` 至少覆盖以下来源中的可用项：

- 好评率
- 评论总数
- 当前在线人数
- 标签
- 多人模式
- 简介

每条证据都必须包含：

- `label`
- `value`
- `interpretation`

禁止只展示原始数字，不解释该数字为什么重要。

### 6.4 评论摘录证据

第一阶段展示 1 到 3 条评论摘录。

选择规则：

- 优先至少选 1 条正向评论支撑亮点
- 如果存在明显负向评论，优先再选 1 条支撑风险
- 如果评论样本不足，可只展示 1 条
- 不做评论聚类和复杂主题建模

每条评论摘录都需要补一句解释，说明它在支撑什么判断。

## 7. 生成流水线

### 7.1 输入

完整分析只基于本地已同步元数据：

- `positiveReviewPct`
- `totalReviews`
- `currentPlayers`
- `tags`
- `multiplayerModes`
- `shortDescription`
- `reviewSnippets`
- 已有的 `recommendationScore`

第一阶段不额外发起新的 Steam 评论抓取来生成报告。

### 7.2 流水线步骤

1. 读取当前游戏的 `GameCard`
2. 规则层预处理基础事实
3. 规则层计算 5 个固定维度分数
4. 规则层生成：
   - 候选亮点
   - 候选风险
   - 结构化证据条目
   - 评论摘录证据
   - `confidence`
   - 初始 `overallScore`
5. 把规则层结果和原始事实打包给 LLM
6. LLM 仅负责生成更自然的：
   - `overview`
   - `strengths`
   - `risks`
   - 可选的维度 `reason` 文案润色
7. 对 LLM 结果做 JSON 解析与字段校验
8. 成功则返回 `hybrid` 报告
9. 失败则回退到规则层独立输出的 `rule` 报告

### 7.3 LLM 约束

模型不负责自由决定报告结构，只负责在规则层骨架上组织文案。

模型必须遵守：

- 只输出严格 JSON
- 不新增自定义维度
- 不伪造未提供的外部事实
- 不把缺失数据写成确定结论
- 不越权给出“必买”“必入”一类强结论

## 8. 降级策略

降级优先级固定如下：

1. `hybrid`
   - 规则层成功，LLM 也成功
2. `rule`
   - 规则层成功，但 LLM 失败、超时、空响应、或 JSON 解析失败
3. 错误返回
   - 关键数据严重不足，连规则层都无法形成基础报告

系统行为要求：

- 不允许因为 LLM 失败导致详情页 AI 区空白
- 不允许把失败状态伪装成成功报告
- 规则版报告也允许落库缓存

## 9. 详情页交互设计

### 9.1 页面结构

详情页中的 `AI 评估` 标签页升级为：

1. 顶部摘要区
   - 总评
   - 生成时间
   - 数据来源标记：`混合分析` 或 `基础分析`
   - `confidence` 标记
2. 维度评分区
3. “查看完整分析”展开区
4. 完整报告内容区
   - 亮点
   - 风险
   - 结构化证据
   - 评论摘录证据
5. 操作区
   - `重新 AI 评估`

### 9.2 首次进入行为

用户进入某个游戏详情页后：

1. 如果已有缓存报告：
   - 直接显示缓存报告
   - 不自动刷新
2. 如果没有缓存报告：
   - 显示骨架或生成中提示
   - 自动触发生成

### 9.3 手动刷新行为

用户点击 `重新 AI 评估` 后：

- 强制触发重算
- 覆盖最近一次缓存
- 刷新 `aiSummary`
- 刷新 `aiScore`
- 保留当前详情页上下文，不跳页

### 9.4 切换状态要求

- 切换标签不重复请求
- 切换到另一款游戏时，按新的 `appid` 重新判断缓存与自动生成
- 不同游戏的分析状态不能串台

## 10. 前端状态设计

第一阶段不再只依赖全局单个 `assessment`。

详情页需要按 `appid` 维护至少以下状态：

```ts
interface DetailAnalysisState {
  report: GameAnalysisReport | null;
  loading: boolean;
  error: string | null;
  expanded: boolean;
}
```

行为要求：

- `report`
  - 存当前游戏完整报告
- `loading`
  - 区分自动生成和手动刷新中的忙碌态
- `error`
  - 用于展示“数据不足”或请求失败
- `expanded`
  - 控制完整报告展开收起

第一阶段允许该状态先挂在详情页内部，等第二阶段再决定是否抽成共享 store。

## 11. 后端命令设计

建议把“读取缓存”和“触发重算”分成两个命令。

### 11.1 读取缓存

`get_game_analysis(appid)`

职责：

- 读取某个游戏最近一次完整分析报告
- 如果没有缓存则返回空
- 不触发生成

### 11.2 生成报告

`generate_game_analysis(appid, forceRefresh)`

职责：

- 生成完整分析报告
- 当 `forceRefresh=false` 且已有缓存时，允许直接返回缓存
- 当 `forceRefresh=true` 时，必须重算并覆盖缓存

这样前端可以做到：

- 进入详情页先轻量读缓存
- 无缓存时再触发生成
- 手动刷新时明确走强制重算

## 12. 数据库存储设计

第一阶段继续沿用 `games` 表补列方案，不新建历史表。

新增字段：

- `ai_analysis_report_json`
- `ai_analysis_generated_at`

字段语义：

- `ai_analysis_report_json`
  - 存完整 `GameAnalysisReport` 的 JSON
- `ai_analysis_generated_at`
  - 存最近一次成功生成报告的时间

迁移策略沿用现有 `add_games_column_if_missing(...)` 模式。

## 13. 轻量字段回写策略

完整报告成功生成后，需要继续回写轻量字段，保证列表和推荐卡片仍能正常工作。

回写规则：

1. `aiSummary`
   - 优先取 `overview`
   - 必要时截断成适合列表显示的简版短评
2. `aiScore`
   - 取 `overallScore`
3. `recommendationScore`
   - 继续走现有综合评分逻辑，使用最新 `aiScore` 参与计算

这样可以确保：

- 首页和列表不会被本次改造破坏
- 第二阶段推荐助手无需等待完整报告展开，也能先显示短评和分数

## 14. 数据不足判断

系统在以下情况下允许生成低置信度报告：

- 缺少评论摘录，但有好评率、评论数、标签、多人模式
- 缺少当前在线，但有足够的口碑与标签信息

系统在以下情况下应返回“数据不足，暂时无法分析”：

- 关键结构化信息严重缺失，无法形成基本判断
- `tags`、`multiplayerModes`、评论与口碑数据都不足以支撑结论

第一阶段禁止为了“看起来完整”而硬生成空洞报告。

## 15. 测试设计

### 15.1 后端规则层测试

覆盖：

- 固定维度评分计算
- `overallScore` 计算
- `confidence` 判定
- 结构化证据生成
- 评论摘录选择
- 数据不足分支

### 15.2 后端命令与持久化测试

覆盖：

- 首次无缓存生成
- 已有缓存读取
- 强制刷新覆盖缓存
- `ai_analysis_report_json` 读写
- `ai_analysis_generated_at` 更新时间
- `aiSummary` / `aiScore` 回写

### 15.3 LLM 失败降级测试

覆盖：

- 超时
- 空响应
- 非法 JSON
- 解析失败

预期结果：

- 返回规则版报告
- 可正常展示
- 可正常缓存

### 15.4 前端交互测试

覆盖：

- 首次进入自动生成
- 有缓存直接展示
- 展开/收起完整报告
- 手动刷新
- 切换到不同游戏时状态隔离

### 15.5 展示完整性检查

覆盖：

- 有评论与无评论
- 有在线人数与无在线人数
- `hybrid` 与 `rule` 两种来源
- `high` / `medium` / `low` 三种置信度

## 16. 第一阶段验收标准

以下条件全部满足，才视为第一阶段完成：

1. 进入详情页 AI 标签时，无缓存会自动生成首份报告
2. 有缓存时会直接显示最近一次报告
3. 报告包含：
   - 总评
   - 固定维度评分
   - 亮点
   - 风险
   - 结构化证据
   - 评论摘录证据
4. 用户可以手动点击 `重新 AI 评估` 强制刷新
5. LLM 失败时，页面仍能展示规则版报告
6. 数据严重不足时，页面显示明确提示，而不是空白或伪造结果
7. 首页、列表、侧栏继续能使用轻量 `aiSummary` / `aiScore`

## 17. 第二阶段复用路径

推荐助手第二阶段直接消费 `GameAnalysisReport`，而不是重新设计单游戏分析。

复用方式：

1. 推荐排序阶段
   - 结合用户需求与 `dimensionScores` 做匹配
2. 推荐解释阶段
   - 复用 `overview`
   - 复用 `strengths`
   - 复用 `risks`
   - 复用 `evidence`
   - 复用 `reviewEvidence`
3. 冷启动候选项
   - 如果某个候选游戏没有完整报告，则按需补生成
   - 不要求先为全库批量生成

这样第二阶段的工作重点只剩：

- 用户意图解析
- 候选集筛选
- 匹配排序
- 推荐解释组合

而单游戏分析逻辑不会重复建设。

## 18. 实施建议

推荐实现顺序：

1. 后端新增报告结构与规则层生成器
2. 数据库补列与读写
3. 命令层拆分缓存读取与强制生成
4. 前端详情页状态改造
5. 详情页 AI 标签展示升级
6. 回写轻量字段并确认列表不受影响
7. 补齐测试

## 19. 设计结论

第一阶段 AI 分析功能应被实现为：

- 一个以单游戏为中心的完整分析引擎
- 一个可缓存最近结果的证据型报告系统
- 一个以规则层为骨架、以 LLM 为文案增强的混合分析流程
- 一个可被第二阶段推荐助手直接复用的数据生产能力

这比继续扩展当前薄型 `AiAssessment` 更清晰，也更适合后续演进。
