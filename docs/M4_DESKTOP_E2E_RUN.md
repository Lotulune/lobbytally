# M4 Windows 原生桌面 E2E 运行记录

- 时间：2026-07-16 14:22 +08:00
- 结果：PASS，`7/7`
- Git commit：`3a0a6b192fc6b2326fd14c5b90c84b07cbfaac31`
- Git worktree dirty：`true`
- Tauri Driver：`2.0.6`
- WebView2：`150.0.4078.65`
- Microsoft EdgeDriver：`150.0.4078.65`
- Tauri Driver SHA-256：`ff57cbb4d7db4824a5c2391ea1bc10f252f0c736653abd0fe861ed133e3f34a6`
- EdgeDriver SHA-256：`735a749df7538eeb15acb116b2b5307a8c0b01c8f606167f84c6702911847719`

## 已验证流程

1. 完成首次偏好引导，重启原生桌面进程后从客户端 SQLite 恢复状态。
2. 加载四个推荐分区，并检查每个候选都有推荐理由。
3. 执行 PRD 7.2 自然语言推荐，确认无 Provider 时显示明确 fallback。
4. 显示 recent/upcoming 日历及早期数据说明。
5. 提交“不感兴趣”反馈，确认刷新后排序结果发生变化。
6. 在 1024x640 和 1280x800 下检查横向溢出、顶部导航遮挡及关键控件边界。
7. 停止服务端后继续显示带数据时间的离线快照。

成功运行后 `e2e-tests/.runtime` 以及系统临时目录中的隔离服务端数据库、客户端 SQLite 和 WebView2 数据目录均不存在。

## 截图证据

| 尺寸 | 文件 | SHA-256 |
| --- | --- | --- |
| 1024x640 | `e2e-tests/artifacts/layout-1024x640.png` | `a01bf01d35eafc648acba7cb14265aa011f2e554e862be8e81abfe6e06eb08e1` |
| 1280x800 | `e2e-tests/artifacts/layout-1280x800.png` | `d9f526fcf7f359d7b1d787de7ca0ebfbc8d4bd8fc1928b9a33f29b17b3ad42c4` |

本记录证明 **Windows 本机** 原生桌面 E2E。跨平台补充：

- CI commit `5e0274b` 上 **Native desktop E2E (Windows)** 与 **Native desktop E2E (Linux)** 均 success，见 [`M4_CI_RUN.md`](M4_CI_RUN.md) 与 run [29497583493](https://github.com/Lotulune/mpgs/actions/runs/29497583493)。
- 制品 `desktop-e2e-Windows` / `desktop-e2e-Linux` 已上传（含诊断与截图）。
- macOS 不做 WebDriver GUI E2E；APP bundle 见 CI `Tauri bundle smoke (macOS)`。
- 安装器安装后启动见 [`M4_INSTALLER_LAUNCH_RUN.md`](M4_INSTALLER_LAUNCH_RUN.md)。
