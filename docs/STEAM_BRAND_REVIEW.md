# Steam 品牌与数据源使用审查（M6）

**目的**：在 MVP 发布前固定品牌、素材与 API 使用边界，避免把“商店链接”做成仿冒 Steam 客户端或未授权品牌使用。

## 1. 允许

- 使用**标准 Steam 商店 URL** / `steam://` 协议让用户在系统浏览器或 Steam 客户端打开对应 AppID（PRD FR-010）。
- 展示**游戏名称、AppID、公开商店元数据**（评价摘要、价格、平台等）用于推荐与证据，并标注来源与抓取时间。
- 在 UI 文案中说明数据来源于 Steam 公开信息与经批准的适配器（见 [DATA_STORAGE.md](DATA_STORAGE.md)、[SOURCES.md](SOURCES.md)）。

## 2. 禁止（MVP）

- 使用 Valve/Steam **徽标、视觉识别系统或仿 Steam 客户端壳** 作为 MPGS 主品牌。
- 抓取或再分发第三方素材站（预告片、关键艺术）作为内置资源。
- 在客户端嵌入 Steam Web API Key 或代理用户 Steam 登录/密码。
- 暗示 MPGS 为 Valve 官方产品或“Steam 官方推荐”。

## 3. 当前客户端主题

多主题皮肤（复古电子、极简、MC 方块、Steam 商店风格、和风）为**原创程序化 UI**，不包含 Valve 商标素材。名为 “Steam 商店” 的主题是布局/配色启发，发布文案应避免使用受保护徽标图。

## 4. API 与条款

- 官方 Web API 使用须遵守 [Steam Web API Terms](https://steamcommunity.com/dev/apiterms)（Key 保密、配额、用户数据最小必要）。
- 商店 HTML 搜索为**易变适配器**，限流、失败与合规回退见 M1/SOURCES；上线前再次核验条款变更日期。

## 5. 审查签字

| 检查项 | 结果 | 备注 |
| --- | --- | --- |
| 客户端无 Steam/AI Key | _待复核_ | Tauri capabilities 最小权限；CSP connect-src 本机 API |
| 无 Valve 徽标资源入库 | _待复核_ | icons 为 MPGS 自有 |
| 商店跳转仅为标准链接 | _待复核_ | opener 白名单 store.steampowered.com / steam:// |
| 数据源文档与限流 | _待复核_ | SOURCES + dbtool 间隔 |
| 对外文案无官方背书暗示 | _待复核_ | README/安装器文案 |

| 项目 | 内容 |
| --- | --- |
| 审查人 | _待填_ |
| 日期 | _待填_ |
| 结论 | 有条件通过 / 阻断发布 |
