// Operator data-pipeline dashboard. Requires the server admin token.
// Surfaces inventory, M7 section coverage, refresh tasks, and app-id lookup
// so ingestion health is not a black box.

import { useCallback, useEffect, useMemo, useState } from "react";
import { ApiError } from "../api/client";
import type { DataStatusResponse, PipelineAppPresence } from "../api/types";
import { formatAgo } from "../app/format";
import { apiClient } from "../app/runtime";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";
import { Panel } from "../components/Panel";
import { Skeleton } from "../components/Skeleton";

const ADMIN_TOKEN_KEY = "mpgs.admin_token.v1";

function readToken(): string {
  try {
    return localStorage.getItem(ADMIN_TOKEN_KEY) ?? "";
  } catch {
    return "";
  }
}

function writeToken(token: string) {
  try {
    if (token) localStorage.setItem(ADMIN_TOKEN_KEY, token);
    else localStorage.removeItem(ADMIN_TOKEN_KEY);
  } catch {
    // ignore quota / private mode
  }
}

function msLabel(ms: number | null | undefined): string {
  if (ms == null) return "从未";
  return formatAgo(ms);
}

function ratioLabel(ratio: number | null | undefined): string {
  if (ratio == null || Number.isNaN(ratio)) return "—";
  return `${Math.round(ratio * 1000) / 10}%`;
}

function taskPlainName(name: string): string {
  switch (name) {
    case "catalog_sync":
      return "目录同步 (AppList)";
    case "candidate_collection":
      return "联机候选收集 (商店搜索)";
    case "enrichment":
      return "详情 enrich";
    case "quality_check":
      return "质量检查";
    case "retrieval_sync":
      return "检索索引";
    case "recommendation_telemetry_retention":
      return "推荐遥测清理";
    default:
      return name;
  }
}

export function DataOpsScreen({ onOpenGame }: { onOpenGame?: (appId: number) => void }) {
  const [token, setToken] = useState(readToken);
  const [tokenDraft, setTokenDraft] = useState(readToken);
  const [status, setStatus] = useState<DataStatusResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [lookup, setLookup] = useState("");
  const [presence, setPresence] = useState<PipelineAppPresence | null>(null);
  const [lookupError, setLookupError] = useState<string | null>(null);

  const saveToken = () => {
    const next = tokenDraft.trim();
    writeToken(next);
    setToken(next);
  };

  const load = useCallback(async () => {
    if (!token) {
      setError("请先填写并保存 Admin Token（与服务器 MPGS_ADMIN_TOKEN 相同）。");
      setStatus(null);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const data = await apiClient.adminDataStatus(token);
      setStatus(data);
    } catch (e) {
      const msg =
        e instanceof ApiError
          ? `${e.message}${e.status ? ` (HTTP ${e.status})` : ""}`
          : String(e);
      setError(msg);
      setStatus(null);
    } finally {
      setLoading(false);
    }
  }, [token]);

  useEffect(() => {
    void load();
  }, [load]);

  const inventory = status?.inventory;
  const m7 = status?.m7_coverage;
  const coverage = status?.coverage;
  const tasks = status?.tasks ?? [];

  const pipelineHint = useMemo(() => {
    if (!inventory || !m7) return null;
    if (m7.upcoming_candidates <= 1) {
      return "即将发售有日期的联机候选极少：商店搜索主要按评价排序，新游/评价少的派对作很难进池。";
    }
    if (inventory.released_last_14_days < 30) {
      return "近 14 天有日期的已发售联机作偏少：多半是候选发现偏「高评价老盘」而不是「新发售」。";
    }
    return "覆盖看起来健康；若某款游戏仍缺失，用下方 AppID 查询看它卡在哪一层。";
  }, [inventory, m7]);

  const runLookup = async () => {
    setLookupError(null);
    setPresence(null);
    const raw = lookup.trim();
    if (!raw) {
      setLookupError("输入 Steam AppID 或游戏名关键词。");
      return;
    }
    if (!token) {
      setLookupError("需要 Admin Token。");
      return;
    }
    try {
      if (/^\d+$/.test(raw)) {
        const appId = Number(raw);
        const result = await apiClient.adminAppPresence(token, appId);
        setPresence(result);
      } else {
        const search = await apiClient.search(raw, 8);
        const items = search.items ?? [];
        if (items.length === 0) {
          setLookupError(`公开搜索无结果：「${raw}」。很可能未入库或未 enrich。`);
          return;
        }
        setPresence({
          app_id: items[0].app_id,
          in_apps: true,
          has_multiplayer_profile: null,
          app: {
            app_id: items[0].app_id,
            canonical_name: items[0].name,
            release_date: items[0].release_date,
            release_state: items[0].release_state,
            app_type: null,
          },
          search_hits: items.map((it) => ({
            app_id: it.app_id,
            name: it.name,
            release_date: it.release_date,
          })),
          note: "按名称搜索的是公开索引；精确是否在联机池请点 AppID 再查。",
        });
      }
    } catch (e) {
      setLookupError(e instanceof ApiError ? e.message : String(e));
    }
  };

  return (
    <section className="settings-screen" aria-label="数据管道">
      <header className="screen-head">
        <div>
          <h2>数据管道</h2>
          <p>查看入库库存、刷新任务与 App 是否进池。需要管理员令牌。</p>
        </div>
        <div className="statusline">
          {status?.build_git_sha && <Chip>build {status.build_git_sha.slice(0, 8)}</Chip>}
          {status?.generated_at_ms != null && (
            <Chip>快照 {formatAgo(status.generated_at_ms)}</Chip>
          )}
          <Button size="small" onClick={() => void load()} disabled={loading}>
            {loading ? "刷新中…" : "刷新"}
          </Button>
        </div>
      </header>

      <Panel title="管理员令牌">
        <p className="muted">
          与服务器 <code>MPGS_ADMIN_TOKEN</code> 一致。仅保存在本机浏览器，不会提交到仓库。
        </p>
        <div className="pref-row" style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <input
            type="password"
            value={tokenDraft}
            onChange={(e) => setTokenDraft(e.target.value)}
            placeholder="MPGS_ADMIN_TOKEN"
            style={{ flex: "1 1 240px", minWidth: 200 }}
            autoComplete="off"
          />
          <Button onClick={saveToken}>保存</Button>
        </div>
      </Panel>

      {error && (
        <Panel title="加载失败">
          <p role="alert">{error}</p>
        </Panel>
      )}

      {loading && !status && (
        <div className="feed-grid" aria-busy="true">
          {Array.from({ length: 4 }, (_, i) => (
            <Skeleton key={i} />
          ))}
        </div>
      )}

      {status && inventory && (
        <>
          <Panel title="人话结论">
            <p>{pipelineHint}</p>
            <ul className="muted">
              <li>
                联机池约 <strong>{inventory.multiplayer_profiles}</strong> 个；近 14 天有日期的已发售{" "}
                <strong>{inventory.released_last_14_days}</strong> 个。
              </li>
              <li>
                即将发售总数 {inventory.coming_soon_total}，其中有明确日期{" "}
                {inventory.coming_soon_dated}（日历「有日期」几乎只靠这个）。
              </li>
              <li>
                库内最大发售日 {inventory.max_release_date ?? "—"}
                {inventory.max_release_date_name
                  ? ` · ${inventory.max_release_date_name}`
                  : ""}
                {inventory.max_release_date_app_id != null
                  ? ` (#${inventory.max_release_date_app_id})`
                  : ""}
              </li>
              <li>
                任务队列 pending {inventory.jobs_pending} / leased {inventory.jobs_leased} / dead{" "}
                {inventory.jobs_dead}
              </li>
            </ul>
          </Panel>

          <Panel title="库存">
            <div className="seg" style={{ flexWrap: "wrap", gap: 8 }}>
              <Chip>apps {inventory.apps_total}</Chip>
              <Chip>联机池 {inventory.multiplayer_profiles}</Chip>
              <Chip>已发售有日 {inventory.released_with_date}</Chip>
              <Chip>近14天发售 {inventory.released_last_14_days}</Chip>
              <Chip>即将发售 {inventory.coming_soon_total}</Chip>
              <Chip>即将发售有日 {inventory.coming_soon_dated}</Chip>
              <Chip>unknown 壳 {inventory.unknown_named_stubs}</Chip>
            </div>
          </Panel>

          {m7 && (
            <Panel title="分区候选（与公开 Feed 同一套资格）">
              <div className="seg" style={{ flexWrap: "wrap", gap: 8 }}>
                <Chip>近期正式发售 {m7.recent_release_candidates}</Chip>
                <Chip>即将发售 {m7.upcoming_candidates}</Chip>
                <Chip>人气老游 {m7.popular_legacy_candidates}</Chip>
                <Chip>老牌联机 {m7.classic_legacy_candidates}</Chip>
                <Chip>有封面 {m7.candidates_with_cover}</Chip>
                <Chip>有日期 {m7.candidates_with_date}</Chip>
              </div>
            </Panel>
          )}

          {coverage && (
            <Panel title="联机资料完整度">
              <div className="seg" style={{ flexWrap: "wrap", gap: 8 }}>
                <Chip>评测 {coverage.with_reviews}</Chip>
                <Chip>CCU {coverage.with_ccu}</Chip>
                <Chip>价格 {coverage.with_price}</Chip>
                <Chip>平台 {coverage.with_platforms}</Chip>
                <Chip>语言 {coverage.with_languages}</Chip>
                <Chip>推荐就绪 {coverage.recommendation_ready_profiles}</Chip>
              </div>
            </Panel>
          )}

          <Panel title="刷新任务">
            <div style={{ display: "grid", gap: 10 }}>
              {tasks.map((task) => (
                <div key={task.task_name} className="pref-row">
                  <strong>{taskPlainName(task.task_name)}</strong>
                  <div className="muted" style={{ fontSize: 13 }}>
                    上次成功：{msLabel(task.last_success_at_ms)} · 下次：
                    {msLabel(task.next_run_at_ms)} · 覆盖 {ratioLabel(task.coverage_ratio)}
                    {task.last_error_category ? ` · 错误 ${task.last_error_category}` : ""}
                  </div>
                  {task.cursor_value && (
                    <code style={{ fontSize: 11, wordBreak: "break-all" }}>{task.cursor_value}</code>
                  )}
                </div>
              ))}
            </div>
          </Panel>
        </>
      )}

      <Panel title="App 进池查询">
        <p className="muted">
          例：机械狂欢 Steam 页是 app/4108000。若显示「不在 apps 表」，就是发现阶段没扫到，不是排序把它藏了。
        </p>
        <div className="pref-row" style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <input
            value={lookup}
            onChange={(e) => setLookup(e.target.value)}
            placeholder="AppID 如 4108000 或 名称"
            style={{ flex: "1 1 240px", minWidth: 200 }}
            onKeyDown={(e) => {
              if (e.key === "Enter") void runLookup();
            }}
          />
          <Button onClick={() => void runLookup()}>查询</Button>
        </div>
        {lookupError && <p role="alert">{lookupError}</p>}
        {presence && (
          <div style={{ marginTop: 12 }}>
            <div className="seg" style={{ flexWrap: "wrap", gap: 8 }}>
              <Chip>app {presence.app_id}</Chip>
              <Chip tone={presence.in_apps ? undefined : "danger"}>
                {presence.in_apps ? "已在 apps" : "不在 apps"}
              </Chip>
              {presence.has_multiplayer_profile != null && (
                <Chip tone={presence.has_multiplayer_profile ? undefined : "warn"}>
                  {presence.has_multiplayer_profile ? "在联机池" : "不在联机池"}
                </Chip>
              )}
            </div>
            {presence.app && (
              <p>
                {presence.app.canonical_name}
                {presence.app.release_date ? ` · 发售 ${presence.app.release_date}` : ""}
                {presence.app.release_state ? ` · ${presence.app.release_state}` : ""}
              </p>
            )}
            {presence.note && <p className="muted">{presence.note}</p>}
            {presence.search_hits && presence.search_hits.length > 0 && (
              <ul>
                {presence.search_hits.map((hit) => (
                  <li key={hit.app_id}>
                    <Button
                      size="small"
                      variant="ghost"
                      onClick={() => {
                        setLookup(String(hit.app_id));
                        void (async () => {
                          if (!token) return;
                          try {
                            const result = await apiClient.adminAppPresence(token, hit.app_id);
                            setPresence(result);
                          } catch (e) {
                            setLookupError(e instanceof ApiError ? e.message : String(e));
                          }
                        })();
                      }}
                    >
                      #{hit.app_id} {hit.name}
                      {hit.release_date ? ` (${hit.release_date})` : ""}
                    </Button>
                  </li>
                ))}
              </ul>
            )}
            {onOpenGame && presence.in_apps && (
              <Button size="small" onClick={() => onOpenGame(presence.app_id)}>
                打开详情
              </Button>
            )}
          </div>
        )}
      </Panel>
    </section>
  );
}
