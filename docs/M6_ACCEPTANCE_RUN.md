# M6 acceptance run

- When: 2026-07-17 06:48:04 +08:00
- Result: PASS
- Git commit: `abf56c0a8a3b6903137a53347b01169731f60846`
- Git worktree dirty: `False`
- Acceptance script SHA-256: `590ab7e1e0fe07b72ea37e669c2e62bc291bf9cbb94c136623fd13f9c6a55564`
- Package built: `False`
- Package path: ``
- Passed: 12 / 12

| ID | OK | Detail |
| --- | --- | --- |
| source.clean | yes | git_worktree_dirty=False |
| layout.files | yes | all 17 required paths present |
| licenses.generated | yes | THIRD_PARTY_LICENSES.md bytes=11998 |
| unit.storage_upgrade_backup | yes | upgrade path + backup/restore tests passed |
| unit.server_m6 | yes | meta provenance + soak + fault tests passed |
| build.tools | yes | mpgs-server + mpgs-dbtool built sha=abf56c0a8a3b6903137a53347b01169731f60846 |
| runtime.ready | yes | url=http://127.0.0.1:19930/health/ready live+ready=200 |
| runtime.meta_provenance | yes | service=0.1.0 algo=rules-0.2.0 schema=7 git=abf56c0a8a3b6903137a53347b01169731f60846 data_ms=1784242082675 |
| runtime.feed | yes | status=200 items=3 |
| runtime.nl_fallback | yes | status=200 ai_status=fallback body={"ai_status":"fallback","ai_summary":null,"ai_summary_evidence_ids":[],"algorithm_version":"rules-0.2.0","data_updated_a |
| ops.backup_restore | yes | backup+restore+integrity ok |
| package.provenance | yes | skipped (-Package not set); layout scripts present |

This run proves offline M6 release-hardening gates (docs, packaging layout, soak/fault/upgrade tests, meta provenance, backup/restore).
Code signing, notarization, and production compliance signatures remain human gates (see SIGNING_AND_UPDATES.md / PRIVACY.md).
