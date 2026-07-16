# M6 验收说明

M6（发布加固）按 [MVP_PLAN.md](MVP_PLAN.md#m6发布加固) 与 [PRD §13](PRD.md#13-mvp-发布验收) 验收。  
目标是可运维、可回滚、可追溯的发布基线；**不**在未授权情况下启用真实代码签名或公网部署。

## 退出条件对照

| 条件 | 工程证据 |
| --- | --- |
| 性能 / 长时间运行 / 故障注入 / 备份恢复 / 升级 | `cargo test`：`m6_*` soak/fault、storage 升级路径与 backup/restore；历史 P95 门槛测试（`two_thousand_game_feed_meets_local_p95_gate`，`#[ignore]` 手工） |
| Windows/Linux 服务包；桌面包 | `packaging/**` + `scripts/package_server.ps1`；桌面仍由 CI Tauri smoke（NSIS/DEB/APP） |
| 签名 / 自动更新 / 隐私 / 第三方许可 / Steam 品牌 | [SIGNING_AND_UPDATES.md](SIGNING_AND_UPDATES.md)、[PRIVACY.md](PRIVACY.md)、[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)、[STEAM_BRAND_REVIEW.md](STEAM_BRAND_REVIEW.md) |
| 运维手册 / 回滚 / 已知限制 | [OPERATIONS.md](OPERATIONS.md)、[ROLLBACK.md](ROLLBACK.md)、[KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md) |
| 产物可追溯 | `PROVENANCE.json`、`SHA256SUMS.txt`、`/v1/meta` 的 version/schema/build_git_sha/data_updated_at_ms |
| 合规签字 | 文档内签字栏；由发布负责人完成，脚本只检查文档存在 |

## 本机门禁

```powershell
# 离线全量（要求干净 git 工作树才记 PASS）
.\scripts\m6_acceptance.ps1

# 可选：打本地 release 包（耗时）
.\scripts\m6_acceptance.ps1 -Package
```

结果写入 [`M6_ACCEPTANCE_RUN.md`](M6_ACCEPTANCE_RUN.md)。

## PRD §13 映射

1. Win 主流程 + macOS/Linux 打包冒烟 → 沿用 M4 E2E/CI 证据；M6 不重复 GUI。
2. 四分区/搜索/详情/偏好/反馈/AI → M4/M5 验收 + M6 服务冒烟。
3. AI/Steam/网络不可用 → M5 fallback + M6 fault 测试 + 客户端离线缓存（M4）。
4. 数据质量/黄金/契约/迁移/备份/安全 → storage 测试 + package 布局 + admin 无令牌拒绝。
5. 无 Key/SQL 入客户端 → capabilities + 文档审查项。
6. 版本/算法/快照/签名策略记录 → meta 字段 + PROVENANCE + SIGNING 文档。

## 非本脚本范围

- 购买证书、启用签名 CI、公网部署、生产导入真实用户数据。
- 发布负责人隐私/许可/Steam 品牌**签字**（脚本仅验证模板文件在场）。
