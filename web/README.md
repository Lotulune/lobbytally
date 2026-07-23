# MPGS 桌面客户端（M4）

Tauri 2 + React + TypeScript 客户端。前端在 `web/`，Tauri 壳在 `apps/desktop/src-tauri/`。

## 结构

```text
web/                     React/TS 前端（浏览器可独立开发；也是 Tauri 加载的产物）
  src/
    api/                 类型化 API 客户端、ETag 快照缓存、离线反馈队列
    fx/                  单 rAF 特效引擎、有界粒子池、内建/主题粒子形状
    theme/               五主题定义（token 皮肤 + 特效模块 + 程序化贴图）
    screens/             引导、四分区推荐流、搜索、日历、详情、设置、外壳
    app/                 主题/Toast 上下文、运行时单例、展示格式化、日历/偏好/防抖辅助
    styles/              base.css（token 契约）+ themes.css（五主题皮肤）
  tests/                 vitest：池、主题、api、反馈队列、格式化
apps/desktop/src-tauri/  Tauri 2 壳（独立 cargo workspace，最小权限）
```

## 前置

- Node.js LTS 与 pnpm。
- 运行客户端前先启动服务端（默认 `http://127.0.0.1:17880`，见 `docs/DEVELOPMENT.md`）。
- 打包 Tauri 需要平台 WebView 工具链（Windows: MSVC Build Tools + WebView2；Linux: WebKitGTK 4.1）。

## 浏览器开发

```powershell
pnpm install
# 另开一个终端启动服务端（带演示数据）：
#   $env:MPGS_SEED_DEMO='true'; cargo run -p mpgs-server
pnpm --filter mpgs-web dev     # http://localhost:5173，/v1 代理到 127.0.0.1:17880
```

## 校验

```powershell
pnpm --filter mpgs-web typecheck
pnpm --filter mpgs-web test
pnpm --filter mpgs-web build
# 仓库根：M4 API 级验收（含临时 demo 服务端）
#   .\scripts\m4_acceptance.ps1
# 或：pnpm m4:accept
# 对已有服务会写入会话/偏好/反馈，必须显式授权：
#   .\scripts\m4_acceptance.ps1 -BaseUrl http://127.0.0.1:8080 -AllowExistingServerWrites
```

## Tauri 桌面（可选）

```powershell
# 仓库根已声明 @tauri-apps/cli；首次：pnpm install
pnpm --filter mpgs-web build
pnpm exec tauri dev --config apps/desktop/src-tauri/tauri.conf.json
# 安装包（Windows x64，未签名）：
pnpm exec tauri build --config apps/desktop/src-tauri/tauri.conf.json --ci --no-sign -b nsis
pnpm exec tauri build --config apps/desktop/src-tauri/tauri.conf.json --ci --no-sign -b msi
# 产物：
#   apps/desktop/src-tauri/target/release/mpgs-desktop.exe
#   apps/desktop/src-tauri/target/release/bundle/nsis/MPGS_0.1.0_x64-setup.exe
#   apps/desktop/src-tauri/target/release/bundle/msi/MPGS_0.1.0_x64_en-US.msi
```

`tauri.conf.json` 的 bundle 目标为跨平台 `all`；CI 在原生 runner 上分别用 `--bundles deb`、
`--bundles nsis`、`--bundles app` 做 Linux、Windows、macOS 冒烟。构建成功不等于安装后 GUI
验收，最终 M4 证据要求见 `docs/M4_ACCEPTANCE.md`。

打包构建默认把 API 基址设为 `http://127.0.0.1:8080`，服务端 CORS 白名单已包含
`http://tauri.localhost` / `tauri://localhost`（Windows/其他平台的 webview 源）。
可用 `MPGS_CORS_ALLOWED_ORIGINS` 覆盖。`web/.env` 的 `VITE_MPGS_API_BASE` 仅可设为
`http://127.0.0.1:8080` 或 `http://localhost:8080`；桌面 E2E 模式另使用
`http://127.0.0.1:18080` 隔离本机开发服务。桌面 CSP 只允许这三个本机源，构建会拒绝其他值，
避免生成运行后必然被 CSP 阻断的客户端。

浏览器开发模式保持 `VITE_MPGS_API_BASE` 为空时会走同源 `/v1` 代理。可选的
`VITE_MPGS_DEV_PROXY_TARGET` 可将该代理指向其他本机服务端端口，仅用于开发和本地验收。

## 主题与特效

五个主题：复古电子、极简白线、MC 方块、Steam 商店、樱树和风。每个主题提供一套
设计 token 皮肤（`themes.css` 的 `[data-theme]`）与一个特效模块（环境层动画、点击
反馈、`like/dismiss/confirm/error` 语义动作）。特效走单 rAF 循环 + 有界粒子池，
标签页隐藏时暂停，尊重 `prefers-reduced-motion`，可在顶栏切换 全/低/关。所有贴图
在运行时程序化生成，不加载任何第三方素材。
