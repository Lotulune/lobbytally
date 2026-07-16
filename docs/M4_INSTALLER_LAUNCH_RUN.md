# M4 Windows installer install-and-launch smoke

- When: 2026-07-16 20:32:12 +08:00
- Result: **PASS**
- Git commit: `5e0274b5224f7fe73f7c4160a4aafb1f1a3b6386`
- Installer: `apps/desktop/src-tauri/target/release/bundle/nsis/MPGS_0.1.0_x64-setup.exe`
- Installer SHA-256: `912ff3ee4d31632b90944b999b4895125e97225bc9c949089e1effb9e9569662`
- Silent install: `/S /D=%LOCALAPPDATA%\MPGS-InstallSmoke` exit code `0`
- Installed executable: `%LOCALAPPDATA%\MPGS-InstallSmoke\mpgs-desktop.exe`
- Launch: process `pid=28928` still alive after 10s = `True`, `Responding=True`, main window title starts with `MPGS`
- Isolated data dir via `MPGS_CLIENT_DATA_DIR`: temp `mpgs-install-smoke-data-20260716203157` containing `client-state.sqlite3` (+ shm/wal)
- Uninstall: silent `/S` exit `0`; executable remaining = `False`

This proves the unsigned Windows NSIS package installs, the desktop process starts and creates client SQLite, and uninstall works.
It does not replace signed-release verification or full GUI E2E (covered by native desktop E2E / CI).
