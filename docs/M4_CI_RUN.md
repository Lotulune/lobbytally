# M4 CI 证据（跨平台）

- 时间：2026-07-16（UTC `2026-07-16T12:17:36Z` → `12:30:49Z`）
- 结果：**success（11/11 jobs）**
- Git commit：`5e0274b5224f7fe73f7c4160a4aafb1f1a3b6386`
- Workflow run：https://github.com/Lotulune/mpgs/actions/runs/29497583493

## Job 结果

| Job | 结论 | 链接 |
| --- | --- | --- |
| Web test and build | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618132799) |
| Format, test, and lint | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618133033) |
| Build Linux x64 | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618132992) |
| Build Linux ARM64 | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618132937) |
| Build Windows x64 | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618132850) |
| Build Windows ARM64 | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618132902) |
| Tauri bundle smoke (Linux / DEB) | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618132830) |
| Tauri bundle smoke (Windows / NSIS) | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618132821) |
| Tauri bundle smoke (macOS / APP) | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618132882) |
| Native desktop E2E (Linux) | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618132891) |
| Native desktop E2E (Windows) | success | [job](https://github.com/Lotulune/mpgs/actions/runs/29497583493/job/87618132863) |

## 制品

| Artifact | 用途 |
| --- | --- |
| `desktop-e2e-Linux` | Linux 原生 E2E 诊断/截图 |
| `desktop-e2e-Windows` | Windows 原生 E2E 诊断/截图 |
| `mpgs-linux-x64` / `mpgs-linux-arm64` | 服务端/dbtool 原生二进制 |
| `mpgs-windows-x64` / `mpgs-windows-arm64` | 服务端/dbtool 原生二进制 |

## 覆盖边界

- Linux/Windows 原生桌面 E2E 由 CI 实际执行并通过。
- Linux DEB、Windows NSIS、macOS APP 由 `desktop-smoke` 在对应 runner 上构建通过。
- macOS 无 Tauri 桌面 WebDriver；本记录以 **APP bundle 构建冒烟** 为跨平台证据，不声称 macOS GUI 自动点选。
- Windows **安装器安装后启动** 见本机记录 [`M4_INSTALLER_LAUNCH_RUN.md`](M4_INSTALLER_LAUNCH_RUN.md)，不由本 CI job 覆盖。
