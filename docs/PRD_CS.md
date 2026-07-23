# MPGS C/S 架构强化 PRD

| 字段 | 内容 |
| --- | --- |
| 状态 | 已确认，后端阶段已实现 |
| 目标 | 将桌面客户端与服务端安装、运行和数据边界显式分离 |
| 地址输入 | 默认为空；示例 `https://mpgs.example.com` / `127.0.0.1:17880` |
| 本期实现 | 服务发现 API、远程访问 CORS、后端独立部署 |
| 后续实现 | 桌面端连接引导、地址管理、缓存与凭据隔离 |

## 1. 产品结论

MPGS 桌面应用是纯客户端，不安装、不启动也不管理本机 `mpgs-server`。用户首次启动桌面应用时必须先配置服务地址；连接并确认服务兼容、就绪后，才能进入推荐、搜索、社区和账户功能。

服务端提供两种互不依赖的安装模式：

- `backend`：只安装 API、worker 与本机 SQLite，适合只向桌面客户端提供服务。
- `full`：安装 API、worker 和浏览器 Web UI，桌面客户端仍通过同一 HTTPS API 访问。

两种模式使用同一个 API 契约和数据库，不要求服务端主机安装桌面客户端。生产环境由反向代理终止 TLS，示例公网地址为 `https://mpgs.lunafleur.dpdns.org`。

## 2. 背景与现状

仓库已经具备 Rust/Axum 服务端、Tauri 桌面壳、React Web UI、独立 SQLite 和服务端安装包，逻辑上也规定客户端不得直接访问权威数据库。但当前产品体验仍容易被理解为“同机前后端”：

- 打包客户端不再隐式指向本机端口；本地开发服务端默认 `127.0.0.1:17880`。
- 客户端没有仅凭一个服务地址完成产品识别与兼容性检查的稳定握手接口。
- Compose 的唯一宿主机 HTTP 入口来自 Web 网关，后端容器单独启动时没有可供主机反代的端口。
- 生产 CORS 示例覆盖默认列表后只保留 Web Origin，会阻止 Tauri WebView 访问公网 API。

## 3. 目标与非目标

### 3.1 目标

- 用户能够在桌面端输入服务地址并明确连接到远程 MPGS Server。
- 服务地址在保存前完成身份识别、协议兼容和就绪检查。
- 登录令牌、匿名会话、离线缓存和待同步写入按服务地址隔离。
- 服务端可独立安装，不包含 React 静态站点或桌面程序。
- 运维人员也可选择完整安装，在同一域名提供 API 与 Web UI。
- 后端对 Windows/Linux 服务包与 Docker 部署保持兼容。

### 3.2 非目标

- 本期不实现服务目录、自动局域网发现或账号跨服务迁移。
- 不允许客户端直连 SQLite、管理 API 或内部 worker API。
- 不在 Axum 内置 TLS；生产 TLS 继续由 Nginx 等反向代理终止。
- 不支持在一个桌面窗口中同时聚合多个服务端的数据。
- 不在本期修改 React/Tauri 前端；第 12 节给出前端交接契约。

## 4. 角色与边界

| 角色 | 职责 | 不负责 |
| --- | --- | --- |
| 桌面客户端 | 地址配置、交互、本地凭据和离线缓存 | 数据采集、权威存储、服务进程生命周期 |
| MPGS Server | 认证、目录、推荐、社区、AI、版本化 API | 桌面 UI、客户端本地缓存 |
| Worker | Steam 数据任务与富化，和 API 同机访问 SQLite | 对普通用户暴露接口 |
| Web UI（可选） | 浏览器交互，调用同一 API | 作为桌面客户端的运行依赖 |
| TLS 反向代理 | 公网 HTTPS、证书、可信转发头 | 业务逻辑和数据库访问 |

## 5. 核心用户流程

### 5.1 首次连接

1. 桌面应用显示服务地址输入页；输入框默认为空，仅展示填写示例（如 `https://mpgs.example.com`、`127.0.0.1:17880`），必须由用户自行输入并确认连接。
2. 客户端接受 `https://host[:port]`、裸 `host[:port]`、裸 `IP[:port]`（IP 默认 `http`）；域名上的 `http` 仅开发构建的 loopback 可用。不接受用户信息、查询、片段或非空路径。
3. 客户端规范化地址：主机名小写、IPv6 保留方括号、移除默认端口和末尾 `/`，得到用户确认后的 Origin。
4. 客户端请求 `GET /.well-known/mpgs`，超时建议为 8 秒。
5. 响应必须是 JSON，且 `service=mpgs-server`、`discovery_version=1`、`api_version=v1`；否则不得把普通网站误认为 MPGS 服务。
6. 客户端按响应中的 `readiness_path` 请求就绪状态，再请求 `/v1/meta` 获取动态能力和数据状态。
7. 三步成功后才持久化地址，并创建或恢复该地址命名空间下的会话。
8. 首次连接失败时留在连接页，不创建匿名会话，不进入带在线含义的业务页面。

### 5.2 日常启动与离线

- 已保存地址时，客户端先使用该地址进行后台连接检查。
- 服务暂时不可达时，可进入该服务地址对应的只读离线缓存，并清楚显示离线状态；不得切换到其他服务的缓存。
- 写操作在离线时进入该地址独立的待同步队列，恢复连接后仍需校验当前地址与原始队列地址一致。
- `/.well-known/mpgs` 可用但 `/health/ready` 返回 `503` 时，显示“服务维护中”，不能将其归类为地址错误。

### 5.3 切换服务

1. 用户在设置中选择“更换服务”。
2. 客户端对新地址重复完整连接检查。
3. 在用户确认前不替换当前地址，也不清除旧地址数据。
4. 确认后切换活动命名空间；旧地址的访问令牌不得发送给新地址。
5. 切回旧地址时可以恢复其独立缓存与会话；用户可显式删除某个地址的本地数据。

### 5.4 服务端安装

- 后端模式：启动 `mpgs-server` 与 `mpgs-worker`，将 API 回环端口交给 HTTPS 反向代理，不构建或启动 `mpgs-web`。
- 完整模式：额外启动 `mpgs-web`，由 Web 网关同时转发 API、健康检查、OpenAPI 和服务发现端点。
- 原生 Windows/Linux 发布包始终是后端包，不捆绑前端产物。

## 6. 功能需求

| 编号 | 优先级 | 需求 |
| --- | --- | --- |
| CS-001 | P0 | 桌面端首次进入业务界面前必须配置并验证服务地址。 |
| CS-002 | P0 | 服务端提供不依赖数据库就绪状态的 `GET /.well-known/mpgs`。 |
| CS-003 | P0 | 发现响应只返回相对路径，不信任或拼接代理传入的 Host。 |
| CS-004 | P0 | 生产服务允许 `http://tauri.localhost` 与 `tauri://localhost` 两种桌面 Origin。 |
| CS-005 | P0 | 服务端提供仅后端的 Docker 启动模式和可反代回环端口。 |
| CS-006 | P0 | 完整部署的 Web 网关必须把发现端点转发到 API，而不是返回 SPA HTML。 |
| CS-007 | P0 | 客户端令牌、缓存、ETag 和待同步队列以规范化服务 Origin 为一级命名空间。 |
| CS-008 | P0 | 客户端只把认证令牌发送给当前已验证 Origin，不随跨 Origin 重定向转发。 |
| CS-009 | P1 | 设置页支持测试新地址、切换服务和删除指定服务的本地数据。 |
| CS-010 | P1 | 连接错误区分地址格式、TLS、非 MPGS 服务、协议不兼容、维护中和网络超时。 |

## 7. 服务发现契约

请求：

```http
GET /.well-known/mpgs HTTP/1.1
Accept: application/json
```

成功响应：

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

契约约束：

- 该端点无需认证、无需数据库查询，并带标准 `x-request-id` 与 CORS 响应头。
- `discovery_version` 只在发现文档出现不兼容变化时递增。
- 客户端必须忽略未知字段；不识别的 `discovery_version` 或 `api_version` 必须阻止连接。
- 路径均以 `/` 开始，客户端用已验证的服务 Origin 解析，不接受响应返回的新 Origin。
- 动态能力、AI 可用性、schema 和数据时间继续以 `/v1/meta` 为准。

## 8. 部署需求

### 8.1 Docker 后端模式

```bash
MPGS_DEPLOY_MODE=backend ./deploy/update.sh
curl http://127.0.0.1:18081/.well-known/mpgs
```

组件为 `mpgs-server + mpgs-worker`。容器 API 映射到宿主机回环地址 `${MPGS_API_PORT:-18081}`，公网请求使用 `deploy/mpgs-api-host.nginx.conf` 反代。禁止将 SQLite 目录改为网络共享卷。

### 8.2 Docker 完整模式

```bash
MPGS_DEPLOY_MODE=full ./deploy/update.sh
curl http://127.0.0.1:18082/.well-known/mpgs
```

组件为 `mpgs-server + mpgs-worker + mpgs-web`。宿主机 Nginx 使用 `deploy/mpgs-host.nginx.conf` 反代到 Web 网关，Web 网关将 API 类路径转发给后端。

### 8.3 原生服务包

`scripts/package_server.ps1` 产物只包含 `mpgs-server`、`mpgs-dbtool`、服务定义、环境模板与运维文档。管理员通过 `MPGS_BIND_ADDR` 选择回环监听地址，并自行配置 TLS 反向代理。Web UI 如需部署，作为独立可选产物安装。

## 9. 安全与隐私

- 公网服务必须使用证书有效的 HTTPS；客户端不得提供“忽略证书错误”选项。
- CORS 使用精确 Origin 白名单，不允许 `*`，也不启用 Cookie 凭据。
- 客户端不得把管理员 Token、Steam Key 或内置 AI Key写入桌面配置。
- 服务切换前必须停止旧地址的请求与重试，避免令牌和待同步数据跨服务泄漏。
- 服务地址日志可记录 Origin，但不得记录 URL 中的用户信息、查询参数或认证头；输入校验本身应拒绝这些部分。
- 反向代理只有在覆盖可信转发头后才允许服务端设置 `MPGS_TRUST_PROXY_HEADERS=true`。

## 10. 连接状态与错误语义

| 状态 | 判定 | 客户端行为 |
| --- | --- | --- |
| `invalid_url` | 地址格式或协议不符合要求 | 就地提示并禁止请求 |
| `tls_error` | 证书、主机名或 TLS 握手失败 | 阻止连接，不允许绕过 |
| `not_mpgs` | 发现端点 404、非 JSON 或 `service` 不匹配 | 提示该地址不是 MPGS Server |
| `incompatible` | 发现/API 版本不受支持 | 提示升级客户端或更换服务 |
| `not_ready` | 发现成功、就绪检查为 503 | 显示维护状态并允许重试 |
| `timeout` | 连接或请求超时 | 保留输入，允许重试；已有配置可离线进入 |
| `connected` | 发现、就绪与 meta 均成功 | 保存地址并进入应用 |

服务端不为这些客户端本地状态新增业务错误码；HTTP 状态与发现文档已经足以判定。

## 11. 可观测性与指标

- 反向代理访问日志应能区分 `/.well-known/mpgs`、`/health/ready` 与业务 API。
- 建议统计连接发现成功率、就绪失败率、按服务版本分布和匿名会话创建成功率。
- 不采集用户输入过但未成功验证的完整地址；客户端诊断仅保留错误类别和去敏主机名。

## 12. 交付与验收

### 12.1 后端阶段（本期）

- `GET /.well-known/mpgs` 返回稳定 JSON，并出现在 OpenAPI 3.1 文档中。
- Tauri Origin 的简单请求与预检获得正确 CORS 响应；未知 Origin 不获得 ACAO。
- `backend` 模式不启动 `mpgs-web`，可经 `18081` 健康检查并被 TLS 反代。
- `full` 模式保持现有 Web UI，且发现端点返回 JSON 而非 `index.html`。
- Linux/Windows server 包继续不包含前端构建产物。

### 12.2 前端阶段（交接给后续实现）

- 首次启动页面完成第 5.1 节全流程；地址框默认为空，仅展示填写示例。
- 删除打包环境对本机隐式 API 地址的依赖，并更新 Tauri CSP 以允许用户确认后的 HTTPS/HTTP 服务；实现必须保持最小网络权限，不能简单放开任意协议。
- API client 从持久化的当前服务 Origin 构造 URL，不能在模块加载时固化单一 Base URL。
- IndexedDB/localStorage 中的认证、偏好副本、反馈队列、缓存和 ETag 全部按服务 Origin 分区。
- 自动化测试覆盖首次连接、错误映射、重启恢复、离线进入、服务切换和令牌不跨 Origin。

## 13. Definition of Done

- 后端阶段与前端阶段的验收项全部通过。
- 用户机器只安装桌面客户端即可使用公网服务，不需要运行本机 server。
- 服务主机只安装后端即可服务桌面客户端，不需要安装 Web UI 或桌面端。
- 同一后端也可选择完整部署，且两种模式的业务 API 行为一致。
- 文档、OpenAPI、环境模板和生产反代配置与实际行为一致。

## 14. 已确认决策

| 决策 | 结论 |
| --- | --- |
| 地址示例 | `https://mpgs.example.com`、`127.0.0.1:17880` |
| 客户端是否内置本机服务 | 否 |
| 服务端能否只安装后端 | 能，且作为一等部署模式 |
| Web UI 是否保留 | 保留为可选完整部署 |
| 服务发现路径 | `/.well-known/mpgs` |
| 生产 TLS | 外部反向代理终止 |
| 数据权威 | 服务端本机 SQLite；客户端只走 HTTPS API |

