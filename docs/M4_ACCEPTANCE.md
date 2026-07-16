# M4 验收说明

M4 按 `MVP_PLAN.md` 的原始范围验收，不把未完成项延期，也不以 API smoke 代替桌面端端到端测试。

## 验收层次

| 层次 | 自动化证据 | 能证明什么 | 不能证明什么 |
| --- | --- | --- | --- |
| API 契约 | `scripts/m4_acceptance.ps1` | 7.1/7.2/7.3 API 语义、反馈生效与撤销恢复、严格 ETag `304` | GUI 交互与布局 |
| 客户端逻辑 | Vitest + production build | 离线快照、持久待同步写入、类型与构建 | 真实断网后的桌面体验 |
| 原生打包 | GitHub Actions `desktop-smoke` 矩阵 | Linux/Windows/macOS 能产生原生 bundle | 安装后启动和人工点选 |
| 桌面 E2E | Windows/Linux `tauri-driver` + WebdriverIO | 原生应用重启持久化、7.1/7.2/7.3、反馈刷新、断网缓存、目标尺寸截图 | macOS GUI 自动化、安装器本身 |

只有四层证据均通过，且 PRD 7.1、7.2、7.3 都已端到端验证，才能关闭 M4。

## 运行严格 API 门禁

```powershell
# 推荐：自动创建并清理临时服务和临时数据库
.\scripts\m4_acceptance.ps1

# 对已有服务会写入会话、偏好和反馈，必须显式授权
.\scripts\m4_acceptance.ps1 `
  -BaseUrl http://127.0.0.1:8080 `
  -AllowExistingServerWrites
```

脚本要求：

- 四个分区都非空，且每个推荐条目都有非空推荐理由；
- 反馈响应必须返回有效 `feedback_id`，撤销也必须真正成功；
- `recent`/`upcoming` 日历都非空且发售状态匹配，并包含数据置信度、来源更新时间、日期精度、`review_total` 和布尔型 `early_data`；
- 搜索必须返回与查询词匹配的结果；
- 携带原 ETag 的重复请求必须返回 `304`，返回 `200` 视为失败；
- PRD 7.2 的自然语言接口必须解析人数、时长和合作偏好，返回 3–10 个带理由的候选；未配置外部 AI Provider 时必须明确返回 `ai_status=fallback` 和非空 `fallback_reason`，不能谎报 AI 可用；
- 离线快照、ETag、反馈/偏好/想玩待同步逻辑的指定测试以及完整 Web 测试必须通过；
- Web production build 和 Tauri crate check 必须通过。

无论成功还是运行时失败，脚本都会覆盖 `M4_ACCEPTANCE_RUN.md`。报告包含最终状态、Git commit、dirty 状态、脚本 SHA-256、服务端二进制 SHA-256 以及 API/服务/算法版本；临时数据库路径、设备 ID 和访问令牌不会写入仓库。

## 原生 bundle 冒烟

CI 的 `desktop-smoke` job 在原生 runner 上分别构建：

- Linux：DEB；
- Windows：NSIS；
- macOS：APP。

`tauri.conf.json` 使用跨平台 bundle 配置和 PNG/ICO 图标；各平台 CI 使用 `--bundles` 选择本平台制品。首次 CI 成功前，不能把这些平台标记为已通过。

## 原生桌面 E2E

`e2e-tests/` 驱动编译后的 Tauri 应用，而不是浏览器中的 Vite 页面。Windows/Linux CI 会安装 `tauri-driver` 与平台 WebDriver，启动隔离的演示服务和客户端 SQLite，执行：

- 首次引导，并通过 `reloadSession()` 证明 SQLite 状态跨原生进程重启持久；
- 四个推荐流及每条推荐理由；
- PRD 7.2 自然语言 fallback；
- recent/upcoming 日历及早期数据说明；
- 反馈确认后的列表刷新；
- 1024×640、1280×800 无关键溢出检查和成功截图；
- 停止服务后的离线快照与数据时间。

macOS WKWebView 没有 Tauri 桌面 WebDriver 客户端，因此 macOS 只做原生 APP bundle/启动类证据，不能虚构自动 GUI 点选结果。

## 当前状态

**M4 四层证据已齐（commit `5e0274b`，2026-07-16）。** 正式关闭条件按本节与下表，不再以“待 CI”占位。

| 层次 | 证据 | 状态 |
| --- | --- | --- |
| API 契约 | [`M4_ACCEPTANCE_RUN.md`](M4_ACCEPTANCE_RUN.md) 本机 `30/30` | 通过 |
| 客户端逻辑 | 同脚本内 Vitest + production build；CI `Web test and build` | 通过 |
| 原生打包 | CI `desktop-smoke` Linux DEB / Windows NSIS / macOS APP | 通过，见 [`M4_CI_RUN.md`](M4_CI_RUN.md) |
| 桌面 E2E | 本机 Windows `7/7` + CI Linux/Windows E2E | 通过，见 [`M4_DESKTOP_E2E_RUN.md`](M4_DESKTOP_E2E_RUN.md)、[`M4_CI_RUN.md`](M4_CI_RUN.md) |

附加本机证据：

- 严格验收 `30/30`；根工作区测试、根/桌面 Clippy、Web 测试/构建、桌面 SQLite 重开测试通过。
- unsigned Windows NSIS（SHA-256 `912ff3ee4d31632b90944b999b4895125e97225bc9c949089e1effb9e9569662`）和 MSI（SHA-256 `c4cf9db8253026f0446c4ba2c7986a9752c10dd9090f36891711037fd2050d85`）。
- 安装器静默安装 → 进程启动并创建 `client-state.sqlite3` → 静默卸载：[`M4_INSTALLER_LAUNCH_RUN.md`](M4_INSTALLER_LAUNCH_RUN.md)。
- capability 仅含 core 默认权限和限定 Steam URL 的 opener；构建产物未发现服务端管理令牌或常见 Provider Key 标识。

跨平台 CI（全绿）：https://github.com/Lotulune/mpgs/actions/runs/29497583493 — 11/11 jobs success，含 Linux/Windows 原生 E2E 与三平台 bundle smoke。macOS 无桌面 WebDriver，以 APP bundle 构建冒烟为该平台证据，不声称 GUI 自动点选。

最新 API 结果只认 `M4_ACCEPTANCE_RUN.md`，历史 `21/21` 结果因旧脚本允许 ETag `200`、反馈撤销跳过和搜索零结果而作废。

### 明确不在 M4 关闭范围内的项

- 真实候选深度富化与 `recommendation_ready` 扩量（发布门禁，可与 M5 并行）。
- 代码签名、自动更新、安装器公证（属 M6）。
- macOS 人工 GUI 全流程点选（无官方桌面 WebDriver 时不强制虚构）。
