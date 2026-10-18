# 生产与项目当前状态

## 2026-10-19 云端迁移

当前工作树已部署到 Vercel + Supabase，从空库重新开始；原域名已切换，旧 VPS 上的应用服务已停用并清理。部署步骤见 [VERCEL_SUPABASE.md](VERCEL_SUPABASE.md)。

| 项目 | 已核对状态 |
| --- | --- |
| Supabase | `Lunafleur / lobbytally`，东京 `ap-northeast-1`，项目 `fzywcxyyazlwetvixdzt` |
| 数据库 | 四份迁移已应用；应用 schema 版本 `1`；专用后台角色连接和 readiness 通过 |
| 数据隔离 | 私有 `mpgs` schema、私有 `avatars` bucket；安全 Advisor 无发现 |
| Vercel | `redaiyos-projects / lobbytally`；容器云端构建与东京运行已验证；[正式站点](https://mpgs.lunafleur.dpdns.org) 已通过 HTTPS、页面、静态资源和 API 验收 |
| 新数据 | 从空库开始，不导入旧 SQLite、账号或游戏数据 |
| 定时任务 | Supabase Cron 已启用：采集每分钟、头像清理每 5 分钟；实际 HTTP 和持久化游标推进已验证 |
| 功能验收 | 32 项线上 API 检查通过，涵盖账号、会话、偏好、游戏详情、投票、头像；测试账号与头像对象已清理 |
| 并发修复 | 任务领取、完成和失败已移除不必要的全局写锁；9 项队列回归、2 项运行时测试、10 项 worker 测试、fmt 和 Clippy 均通过；真实采集与头像清理并发请求均返回 200 |
| 最新部署 | `dpl_EgKeKpr4UuLFgmrEPXAfZyz8w2Y2`，2026-10-19 04:23 JST 已提升至 Production，公开 readiness 正常 |
| 数据采集 | 2026-10-19 04:23 JST 抽样为 552 条新采集目录记录；游标持续前进；新游与预告 Feed 已有内容 |
| AI | 默认提供商尚未配置，`ai_available=false` |
| 原域名 | TXT 所有权验证及 CNAME 切换已完成；Vercel `verified=true`、`misconfigured=false`，直接连接 Vercel 的 HTTPS 校验通过 |
| 定时目标域名 | Vault 已改为 `https://mpgs.lunafleur.dpdns.org`；2026-10-19 04:43 JST 的自动批次返回 HTTP 200 |
| 旧 VPS 服务 | 4 个旧容器已移除；`mpgs-update.timer` 已禁用并停止，更新服务未运行；其他应用及共享 Nginx、Docker 正常 |
| 旧 VPS 空间 | 定向清理 106 个镜像、201 个引用及 21 个历史文件，释放 12,605,526,016 字节（约 12.61 GB），可用磁盘约 19.62 GB |
| 保留资料 | 旧最终数据库约 1.71 GB、WAL/SHM、头像、源码和配置保留，不再由旧应用运行进程使用 |

本次原域名切换记录（Cloudflare zone：`lunafleur.dpdns.org`）：

| 类型 | 名称 | 值 |
| --- | --- | --- |
| TXT | `_vercel` | `vc-domain-verify=mpgs.lunafleur.dpdns.org,d5432a73dd9b14ce1af9` |
| CNAME | `mpgs` | `1804af4d900b1a42.vercel-dns-017.com` |

上述记录已生效。旧主机仅在共享 Nginx 中保留指向 Vercel 地址的临时 `302` 跳转，以兼容尚未更新的 DNS 缓存；正式域名和应用运行已不依赖旧 VPS。旧 Nginx 配置备份位于 `/home/ubuntu/mpgs/nginx.before-cloud-retirement.conf`。该 VPS 还承载其他应用，整机不属于本次停用范围。

以下内容是 **2026-08-11 的旧部署历史快照**，其中 SQLite schema `30`、GHCR/VPS 部署和数据量不代表本次云端项目状态。

---

> **本文件是仓库状态的权威摘要。**  
> 里程碑验收文档（M4–M7）是历史证据，描述的是当时工作树是否通过门禁，**不代表今天的线上状态**。  
> 运维细节见 [OPERATIONS.md](OPERATIONS.md)；仍成立的产品/技术边界见 [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md)。

| 字段 | 内容 |
| --- | --- |
| 文档角色 | 当前状态源（Living status） |
| 最近核对 | 2026-08-11（Asia/Shanghai） |
| 代码基线 | `main` @ `476a6c84a85b897b203326381c321f573e7315c8` |
| 线上核对 | 公网 `https://mpgs.lunafleur.dpdns.org` 的 `/v1/meta` 与公开 Feed |

更新规则：合并会影响生产行为、部署路径、schema 或公开产品叙事的 PR 时，应同步改本文件的「阶段」「已具备」「未关闭门禁」或「抽样数字」小节。不要只改代码而让 README / 本文件继续写「本地验收中」。

---

## 1. 一句话阶段

**LobbyTally（内部包名仍大量使用 `mpgs-*`）已进入「公网 Web/API + 持续 Steam worker 的生产运行期」**。  
M0–M7 的主体工程能力已落地；近阶段工作重心是 **数据管线可靠性、读路径性能、部署不中断、推荐/采集质量**，而不是从零搭 MVP。

阶段标签建议对外写：

```text
生产运行中（MVP 0.2 能力基线 + 数据/运维加固）
```

不要再写「仅 M7 本地工程验收、真实数据门禁完全未开」——那是 2026-07 的快照，已被线上事实取代。

---

## 2. 线上服务

| 项 | 值（2026-08-11 核对） |
| --- | --- |
| 公网入口 | `https://mpgs.lunafleur.dpdns.org` |
| 健康 | `/health/live`、`/health/ready` 正常；schema 与代码迁移一致 |
| 构建 | `build_git_sha` 与当时 `main` HEAD 对齐（持续经 GHCR 滚动） |
| 模式 | `demo_mode=false`，`storage_enabled=true` |
| Schema | `30`（仓库 `migrations/0030_*.sql`） |
| 算法代码 | `algorithm_version=rules-0.3.1` |
| 算法配置 | `config_version=rules-0.2.0`（库内激活配置版本，可与代码版本分离） |
| AI | `ai_available=true`，`ai_multi_model=true`；任务路由含 grok-4.5 |
| 部署形态 | GitHub Actions 构建不可变镜像 → GHCR → VPS `deploy/update.sh` / timer 拉取；**禁止在 VPS 编译** |

公开 API 面（节选）：会话/账号、偏好、四分区 Feed、搜索、日历、游戏详情与证据、反馈、自然语言推荐、社区 play-intents、推荐事件、设备 AI 设置。完整契约见 [API.md](API.md) 与线上 `/openapi.json`。

### 2.1 公开 Feed 抽样规模（非审计门禁）

| 分区 | 约 `total`（`limit=1` 抽样） |
| --- | ---: |
| `recent_release` | 2343 |
| `upcoming` | 289 |
| `popular_legacy` | 156 |
| `classic_legacy` | 774 |

说明：

- 数字会随候选发现、富化与资格规则变化，**不是**发布签字用的 DATA-206 审计结果。
- 条目存在不等于「熟人联机画像高置信」；界面仍可能出现 `fit_band=insufficient_data` 或 `recommendation_index=null`。
- 社区 `GET /v1/community/play-intents` 已通，但真实投票量仍接近冷启动。

快速自检：

```bash
curl -sS https://mpgs.lunafleur.dpdns.org/health/ready
curl -sS https://mpgs.lunafleur.dpdns.org/v1/meta
curl -sS 'https://mpgs.lunafleur.dpdns.org/v1/feeds/classic_legacy?limit=3'
```

---

## 3. 工程能力（相对 7 月里程碑，已成立）

下列能力以 `main` 代码 + 线上行为为准，不再依赖「是否重新跑一遍 M7 脚本」：

| 领域 | 状态 |
| --- | --- |
| 四分区确定性推荐 + 理由/风险/证据字段 | 线上可用 |
| 搜索 / 日历 / 详情 / 媒体 | 线上可用 |
| 账号、会话、头像、社区想玩（play-intent） | 已实现；生产有真实账号路径 |
| AI 网关、多模型任务路由、自定义 Key 设备本地 | 已实现；生产 AI 开启 |
| Steam 目录同步、候选发现、富化 worker、租约 | **生产持续运行** |
| 数据监测快照、维度覆盖、worker 队列观测 | 已合入（PR #23 等） |
| 一体化新游入库与保留策略能力 | 已合入（PR #25 等） |
| 摄入通道熔断、部署 lease 恢复、同版本健康失败不乱重启 | 已合入（#26–#31 一带） |
| Feed/搜索读路径索引与投影优化 | 持续合入（#16–#22、#35–#36 等） |
| CI：fmt / clippy `-D warnings` / test / web / Linux 包 / main 发镜像 | 常态绿 |
| Docker Compose + Nginx 反代 + 定时更新 | 生产路径 |

里程碑历史证据仍保留：

- [M4_ACCEPTANCE](M4_ACCEPTANCE.md) / [M5](M5_ACCEPTANCE.md) / [M6](M6_ACCEPTANCE.md) / [M7](M7_ACCEPTANCE.md)（**历史**）
- 开发计划原文：[MVP_PLAN.md](MVP_PLAN.md)（M0–M6 切片定义；顶部应指向本文件）

---

## 4. 近阶段已完成的生产向工作（摘要）

2026-08 上旬起，`main` 合并主题以生产问题为主（完整列表见 GitHub PR）：

1. **数据增长止血与可观测性**（PR #23）：证据幂等、候选顶页/深度爬取拆节奏、`data-status` 走快照。
2. **一体化入库与保留**（PR #25）：新游核心维度可恢复推进；清理/retention 能力入库（生产执行仍须运维窗口）。
3. **部署与 worker 共存**（#20、#24、#26–#28）：lease 恢复、release 对齐 Compose、同版本健康失败不误杀滚动。
4. **读路径**（#16–#19、#21–#22、#35–#36）：Feed/日历/搜索延迟与投影范围。
5. **可靠性**（#29–#31、#37–#39）：管线兼容、摄入熔断、worker 扫描 IO、enrichment 事务锁。
6. **产品排序修复**（#32–#33）：人气老游恢复与推荐对齐、Top 标识与监测。

执行记录型文档：

- 诊断与双 PR 计划原文：[DATA_PIPELINE_NEXT_STEPS.md](DATA_PIPELINE_NEXT_STEPS.md)（**代码侧双 PR 已合并**；文内 08-08 诊断数字是历史基线）
- 采集/排序优化 PRD：[DATA_ACQUISITION_AND_RANKING_OPTIMIZATION_PRD.md](DATA_ACQUISITION_AND_RANKING_OPTIMIZATION_PRD.md)

---

## 5. 仍未关闭的门禁（诚实列表）

这些才是「还没做完 / 不能对外宣称完成」的项：

### 5.1 产品与增长

- 无 Steam OAuth / 好友 / 库交集。
- 无移动端、推送、组队社交。
- 社区冷启动：公开想玩列表互动极少，不能当作社区产品已验证。
- 自然语言与 AI 解释质量未做大规模用户验收。

### 5.2 推荐质量

- 无已冻结的 200+ 人工相关性黄金矩阵驱动线上权重。
- 无 isotonic / 曝光位置校准与分阶段放量。
- `recommendation_index` 语义仍是请求窗口内指数，不是转化概率。
- 高影响联机字段（自建服、私人房等）仍依赖推断 + 黄金集 + 人工校正，覆盖不均。
- 库内 `config_version` 与代码 `algorithm_version` 可能不一致，属预期，但说明配置尚未随 rules-0.3 全量重发。

### 5.3 数据运维

- 评价 / CCU 等维度覆盖需持续用 `/admin/v1/data-status` 与 `m7-data-audit` 观察；**不得**用 7 月本地烟测数字，也不得把 Feed `total` 当作覆盖率。
- 大规模证据清理、`VACUUM`/compact、备份保留收缩必须有 dry-run、备份与维护窗口（见数据管线文档禁区）。
- 单主 SQLite；无 active-active。

### 5.4 分发与合规

- GitHub Releases 桌面安装包仍停留在早期 **MPGS** 预发布（如 `v0.1.0-rebuild2`），**不能**代表当前 LobbyTally 桌面体验。
- 默认 CI **未**启用代码签名私钥；自动更新公钥/商店分发未作为默认路径关闭。
- 隐私/许可/Steam 品牌使用仍需发布负责人签字流程（见 [PRIVACY](PRIVACY.md)、[STEAM_BRAND_REVIEW](STEAM_BRAND_REVIEW.md)、[THIRD_PARTY_LICENSES](THIRD_PARTY_LICENSES.md)）。
- 第三方许可 UNKNOWN 行须在商店分发前清理。

### 5.5 性能

- 生产 Feed 冷读仍可能到数秒级；日历等接口响应体偏大。持续优化中，不设「已达 SLA」声明。

---

## 6. 文档可信度指引

| 优先级 | 来源 | 用途 |
| --- | --- | --- |
| 1 | 线上 `/v1/meta`、公开 Feed、本文件 | **当前状态** |
| 2 | [OPERATIONS.md](OPERATIONS.md)、[API.md](API.md)、近期数据/推荐 PRD | 如何运维与接口行为 |
| 3 | [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md) | 仍成立的边界 |
| 4 | M4–M7 验收与 `*_RUN.md` | **历史证据**；打开前先看文首「历史快照」提示 |
| 5 | [PRD.md](PRD.md) / [PRD_0.2.md](PRD_0.2.md) / [MVP_PLAN.md](MVP_PLAN.md) | 需求与切片定义；阶段完成度以本文件为准 |

若本文件与里程碑验收冲突：**以本文件 + 线上实测为准**，并考虑去修正过期验收文的文首说明，而不是假装线上倒退。

---

## 7. 维护清单（改状态时）

- [ ] 更新「最近核对」日期与 `build_git_sha`（若需要钉死基线）
- [ ] 刷新 Feed 抽样或明确「仅供参考」
- [ ] 调整「已具备 / 未关闭门禁」
- [ ] 同步 [README.md](../README.md) 顶部「当前状态」一段（保持短链到本文件）
- [ ] 若关闭了分发/签名/审计门禁，从 §5 删掉对应条并留下日期
