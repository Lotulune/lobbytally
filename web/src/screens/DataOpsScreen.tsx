// 数据监测：库存、完整度、任务进度、查一款游戏卡在哪。

import { useCallback, useEffect, useMemo, useState } from "react";
import { ApiError } from "../api/client";
import type { DataStatusResponse, PipelineAppPresence } from "../api/types";
import { formatAgo } from "../app/format";
import { apiClient } from "../app/runtime";
import { Button } from "../components/Button";
import { Panel } from "../components/Panel";
import { Skeleton } from "../components/Skeleton";

const ADMIN_TOKEN_KEY = "mpgs.admin_token.v1";
const LIVE_REFRESH_MS = 5_000;

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
    /* ignore */
  }
}

function pct(n: number, d: number): number {
  if (d <= 0) return 0;
  return Math.max(0, Math.min(100, (n / d) * 100));
}

function toneForPct(p: number): "ok" | "warn" | "bad" {
  if (p >= 85) return "ok";
  if (p >= 40) return "warn";
  return "bad";
}

function ago(ms: number | null | undefined): string {
  if (ms == null) return "还没成功跑过";
  return formatAgo(ms);
}

function taskLabel(name: string): string {
  switch (name) {
    case "catalog_sync":
      return "扫全库名单";
    case "candidate_collection":
      return "找联机游戏";
    case "candidate_top_refresh":
      return "刷新候选顶页";
    case "candidate_continuation":
      return "推进候选深游标";
    case "enrichment":
    case "candidate_enrichment":
      return "补发售日与详情";
    case "enrich_catalog":
      return "采集候选资料";
    case "candidate_discovery":
      return "推进候选发现";
    case "quality_check":
      return "质量自检";
    case "retrieval_sync":
      return "更新搜索索引";
    case "pipeline_snapshot":
      return "完整覆盖率快照";
    case "recommendation_telemetry_retention":
      return "清理旧日志";
    default:
      return name;
  }
}

function runProgress(notes: string | null | undefined): { current: number; total: number } | null {
  if (!notes) return null;
  const fields = new Map(
    notes.split(";").map((part) => {
      const split = part.indexOf("=");
      return split < 0 ? [part, ""] : [part.slice(0, split), part.slice(split + 1)];
    }),
  );
  const current = Number(fields.get("apps_attempted"));
  const total = Number(fields.get("apps_total"));
  if (!Number.isFinite(current) || !Number.isFinite(total) || total <= 0) return null;
  return { current: Math.max(0, current), total: Math.max(1, total) };
}

function activeRunDetail(run: DataStatusResponse["latest_runs"][number] | null): string | null {
  if (!run?.notes) return null;
  const fields = new Map(
    run.notes.split(";").map((part) => {
      const split = part.indexOf("=");
      return split < 0 ? [part, ""] : [part.slice(0, split), part.slice(split + 1)];
    }),
  );
  const attempted = Number(fields.get("apps_attempted"));
  const total = Number(fields.get("apps_total"));
  if (Number.isFinite(attempted) && Number.isFinite(total) && total > 0) {
    const details = ([
      ["store", "商店"],
      ["reviews", "评价"],
      ["popular_reviews", "热评"],
      ["ccu", "在线"],
    ] as const)
      .map(([key, label]) => {
        const value = Number(fields.get(key));
        return Number.isFinite(value) ? `${label} ${value}` : null;
      })
      .filter((value): value is string => value !== null);
    return `正在处理第 ${attempted} / ${total} 款${details.length ? ` · ${details.join(" · ")}` : ""}`;
  }
  if (run.task_type === "candidate_top_refresh") return "正在刷新近期发售与即将发售候选";
  if (run.task_type === "candidate_discovery") return "正在推进 Steam 候选发现游标";
  if (run.task_type === "catalog_sync") return "正在同步 Steam 应用目录";
  return null;
}

function nextRunLabel(nextRunAtMs: number | null, nowMs: number): string {
  if (nextRunAtMs == null) return "未安排下次运行";
  const remaining = nextRunAtMs - nowMs;
  if (remaining <= 0) return "等待调度";
  if (remaining < 60_000) return `${Math.ceil(remaining / 1_000)} 秒后`;
  return `${Math.ceil(remaining / 60_000)} 分钟后`;
}

/** Prefer plain status over raw engine names. */
function taskStatusLine(task: {
  last_success_at_ms: number | null;
  next_run_at_ms: number | null;
  last_error_category: string | null;
}): string {
  if (task.last_error_category) return `上次失败：${task.last_error_category}`;
  return `上次成功 ${ago(task.last_success_at_ms)}`;
}

function Bar({
  name,
  value,
  max,
  suffix,
  tone,
}: {
  name: string;
  value: number;
  max: number;
  suffix?: string;
  tone?: "ok" | "warn" | "bad";
}) {
  const p = pct(value, max);
  const t = tone ?? toneForPct(p);
  return (
    <div className="bar-row">
      <div className="bar-meta">
        <span className="name">{name}</span>
        <span className="nums">
          {value.toLocaleString()}
          {suffix ?? (max > 0 ? ` / ${max.toLocaleString()}` : "")}
          {max > 0 ? ` · ${Math.round(p)}%` : ""}
        </span>
      </div>
      <div className="bar-track" aria-hidden="true">
        <div className="bar-fill" data-tone={t} style={{ width: `${p}%` }} />
      </div>
    </div>
  );
}

function Stat({
  label,
  value,
  hint,
  tone,
}: {
  label: string;
  value: string | number;
  hint?: string;
  tone?: "ok" | "warn" | "bad";
}) {
  return (
    <div className="data-ops-stat" data-tone={tone}>
      <span className="label">{label}</span>
      <span className="value">{typeof value === "number" ? value.toLocaleString() : value}</span>
      {hint ? <span className="hint">{hint}</span> : null}
    </div>
  );
}

export function DataOpsScreen({ onOpenGame }: { onOpenGame?: (appId: number) => void }) {
  const [token, setToken] = useState(readToken);
  const [tokenDraft, setTokenDraft] = useState(readToken);
  const [status, setStatus] = useState<DataStatusResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [lastRefreshAt, setLastRefreshAt] = useState<number | null>(null);
  const [lookup, setLookup] = useState("");
  const [presence, setPresence] = useState<PipelineAppPresence | null>(null);
  const [lookupError, setLookupError] = useState<string | null>(null);

  const saveToken = () => {
    const next = tokenDraft.trim();
    writeToken(next);
    setToken(next);
  };

  const load = useCallback(async (silent = false) => {
    if (!token) {
      setError("请先保存管理密钥。");
      setStatus(null);
      return;
    }
    if (!silent) setLoading(true);
    setError(null);
    try {
      setStatus(await apiClient.adminDataStatus(token));
      setLastRefreshAt(Date.now());
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
      if (!silent) setStatus(null);
    } finally {
      if (!silent) setLoading(false);
    }
  }, [token]);

  useEffect(() => {
    void load(false);
    if (!token) return;
    const interval = window.setInterval(() => {
      if (document.visibilityState === "visible") void load(true);
    }, LIVE_REFRESH_MS);
    return () => window.clearInterval(interval);
  }, [load, token]);

  const inv = status?.inventory;
  const m7 = status?.m7_coverage;
  const cov = status?.coverage;
  const dimensions = status?.dimension_coverage;
  const tasks = status?.tasks ?? [];
  const latestRuns = status?.latest_runs ?? [];
  const workerQueue = status?.worker_queue;
  const integrated = status?.integrated_ingestion;
  const pool = dimensions?.candidates ?? inv?.multiplayer_profiles ?? 0;
  const releasedPool = dimensions?.released_candidates ?? pool;

  const sectionStats = useMemo(() => {
    if (!m7) return [];
    return [
      { name: "最近正式发售", value: m7.recent_release_candidates },
      {
        name: "即将发售（有日期）",
        value: m7.upcoming_candidates,
        tone: m7.upcoming_candidates <= 2 ? ("bad" as const) : undefined,
      },
      { name: "人气老游戏", value: m7.popular_legacy_candidates },
      { name: "经典联机", value: m7.classic_legacy_candidates },
    ];
  }, [m7]);
  const reportedRun = latestRuns.find((run) => run.status === "running") ?? null;
  const activeRun = workerQueue === undefined || workerQueue.leased > 0 ? reportedRun : null;
  const activeProgress = runProgress(activeRun?.notes);
  const activeJob = workerQueue?.active_jobs[0] ?? null;
  const workerRunning = activeRun !== null || (workerQueue?.leased ?? 0) > 0;
  const pendingJobs = workerQueue?.pending ?? inv?.jobs_pending ?? 0;
  const leasedJobs = workerQueue?.leased ?? inv?.jobs_leased ?? 0;
  const dueJobs = workerQueue?.pending_due ?? pendingJobs;

  const runLookup = async () => {
    setLookupError(null);
    setPresence(null);
    const raw = lookup.trim();
    if (!raw) {
      setLookupError("输入游戏编号或名称");
      return;
    }
    if (!token) {
      setLookupError("需要管理密钥");
      return;
    }
    try {
      if (/^\d+$/.test(raw)) {
        setPresence(await apiClient.adminAppPresence(token, Number(raw)));
        return;
      }
      const search = await apiClient.search(raw, 8);
      const items = search.items ?? [];
      const first = items[0];
      if (!first) {
        setLookupError("搜不到。多半还没入库。");
        return;
      }
      setPresence({
        app_id: first.app_id,
        in_apps: true,
        has_multiplayer_profile: null,
        app: {
          app_id: first.app_id,
          canonical_name: first.name,
          release_date: first.release_date,
          release_state: first.release_state,
          app_type: null,
        },
        search_hits: items.map((it) => ({
          app_id: it.app_id,
          name: it.name,
          release_date: it.release_date,
        })),
        note: "点下面的编号可查是否进了联机推荐池。",
      });
    } catch (e) {
      setLookupError(e instanceof ApiError ? e.message : String(e));
    }
  };

  return (
    <section className="data-ops settings-screen" aria-label="数据监测">
      <header className="data-ops-head">
        <h2>数据监测</h2>
        <div className="data-ops-token">
          <input
            type="password"
            value={tokenDraft}
            onChange={(e) => setTokenDraft(e.target.value)}
            placeholder="管理密钥"
            autoComplete="off"
          />
          <Button size="small" onClick={saveToken}>
            保存
          </Button>
          <Button size="small" onClick={() => void load(false)} disabled={loading}>
            {loading ? "…" : "刷新"}
          </Button>
        </div>
        {token && status ? (
          <span className="data-ops-live" aria-live="polite">
            <span className="live-dot" />
            每 5 秒更新{lastRefreshAt ? ` · ${new Date(lastRefreshAt).toLocaleTimeString()}` : ""}
          </span>
        ) : null}
      </header>

      {error ? (
        <p className="data-ops-alert" role="alert">
          {error}
        </p>
      ) : null}

      {loading && !status ? (
        <div className="data-ops-grid" aria-busy="true">
          {Array.from({ length: 4 }, (_, i) => (
            <Skeleton key={i} />
          ))}
        </div>
      ) : null}

      {status && inv ? (
        <>
          <div className="data-ops-grid">
            <Stat
              label="Worker 采集范围"
              value={pool}
              hint={`联机档案 ${inv.multiplayer_profiles.toLocaleString()}，已排除不参与采集的壳数据`}
            />
            <Stat
              label="近两周新发售"
              value={inv.released_last_14_days}
              hint="有明确发售日"
              tone={inv.released_last_14_days < 20 ? "warn" : "ok"}
            />
            <Stat
              label="即将发售·有日期"
              value={inv.coming_soon_dated}
              hint={`共 ${inv.coming_soon_total} 个即将发售`}
              tone={inv.coming_soon_dated <= 1 ? "bad" : "warn"}
            />
            <Stat
              label="名单里最新发售日"
              value={inv.max_release_date ?? "—"}
              hint={inv.max_release_date_name ?? undefined}
            />
            <Stat label="库内游戏总数" value={inv.apps_total} hint="含未整理壳数据" />
            <Stat
              label="排队任务"
              value={pendingJobs + leasedJobs}
              hint={
                `待领取 ${pendingJobs} · 可立即领取 ${dueJobs} · 已租约 ${leasedJobs}`
              }
              tone={inv.jobs_dead_recent > 0 ? "warn" : undefined}
            />
            <Stat
              label="新游入库队列"
              value={(integrated?.pending ?? 0) + (integrated?.retry ?? 0) + (integrated?.leased ?? 0)}
              hint={
                integrated
                  ? `商店 ${integrated.store_details} · 评价 ${integrated.review_summary} · 热评 ${integrated.popular_reviews} · 在线 ${integrated.ccu}`
                  : undefined
              }
              tone={(integrated?.retry ?? 0) > 0 ? "warn" : undefined}
            />
            <Stat
              label="新游入库 Dead"
              value={integrated?.dead ?? 0}
              hint={
                integrated?.oldest_dead_at_ms
                  ? `最早一条 ${ago(integrated.oldest_dead_at_ms)}`
                  : "没有待人工处理的条目"
              }
              tone={(integrated?.dead ?? 0) > 0 ? "bad" : "ok"}
            />
          </div>

          <Panel title="Worker 实时状态">
            <div className="worker-live" data-running={workerRunning ? "true" : "false"}>
              <div className="worker-live-head">
                <span className="worker-state-dot" />
                <div>
                  <strong>
                    {activeRun
                      ? `当前工作：${taskLabel(activeRun.task_type)}`
                      : activeJob
                        ? `当前工作：${taskLabel(activeJob.task_type)}`
                        : "当前空闲"}
                  </strong>
                  <span>
                    {activeRun
                      ? activeRunDetail(activeRun) ?? `开始于 ${ago(activeRun.started_at_ms)}`
                      : activeJob
                        ? `任务 ${activeJob.entity_key} · 第 ${activeJob.attempts} / ${activeJob.max_attempts} 次尝试`
                      : "队列有任务时会自动领取"}
                  </span>
                </div>
                <span className="worker-requests">
                  {activeRun
                    ? `请求 ${activeRun.request_count} · 成功 ${activeRun.success_count}`
                    : workerRunning
                      ? "已领取，正在启动"
                      : "没有执行中的租约"}
                </span>
              </div>
              {activeRun && activeProgress ? (
                <div className="worker-progress">
                  <div className="bar-meta">
                    <span className="name">当前批次</span>
                    <span className="nums">
                      {activeProgress.current.toLocaleString()} / {activeProgress.total.toLocaleString()} 款
                    </span>
                  </div>
                  <div
                    className="bar-track"
                    role="progressbar"
                    aria-valuemin={0}
                    aria-valuemax={activeProgress.total}
                    aria-valuenow={activeProgress.current}
                  >
                    <div
                      className="bar-fill"
                      data-tone="ok"
                      style={{ width: `${pct(activeProgress.current, activeProgress.total)}%` }}
                    />
                  </div>
                </div>
              ) : null}
              <div className="worker-queue-line">
                <span title="已经进入任务队列、尚未被 worker 领取">待领取 {pendingJobs}</span>
                <span title="已到执行时间，可以立即被 worker 领取">可立即领取 {dueJobs}</span>
                <span title="worker 已取得限时执行权，正常情况下就是处理中">已租约 {leasedJobs}</span>
                <span data-tone={inv.jobs_dead_recent > 0 ? "bad" : "ok"}>
                  近 7 天失败 {inv.jobs_dead_recent}
                </span>
              </div>
              <div className="worker-queue-line">
                <span>新游入库待处理 {integrated?.pending ?? 0}</span>
                <span>新游重试 {integrated?.retry ?? 0}</span>
                <span>新游处理中 {integrated?.leased ?? 0}</span>
                <span data-tone={(integrated?.dead ?? 0) > 0 ? "bad" : "ok"}>
                  新游 Dead {integrated?.dead ?? 0}
                </span>
              </div>
            </div>
          </Panel>

          {(integrated?.dead ?? 0) > 0 ? (
            <Panel title="新游入库 Dead 明细">
              <div className="task-list">
                <div className="task-card">
                  <div className="task-top">
                    <span className="title">按阶段</span>
                    <span className="when">
                      {integrated?.dead_by_stage
                        .map((item) => `${item.key} ${item.count}`)
                        .join(" · ")}
                    </span>
                  </div>
                  <div className="task-top">
                    <span className="title">按错误类型</span>
                    <span className="when">
                      {integrated?.dead_by_category
                        .map((item) => `${item.key} ${item.count}`)
                        .join(" · ")}
                    </span>
                  </div>
                </div>
                {integrated?.recent_dead.map((item) => (
                  <div className="task-card" key={`${item.app_id}-${item.stage}`}>
                    <div className="task-top">
                      <span className="title">
                        App {item.app_id} · {item.stage}
                      </span>
                      <span className="when">
                        {item.error_category ?? "unknown"} · {ago(item.dead_at_ms)}
                      </span>
                    </div>
                    {item.error_summary ? <span className="when">{item.error_summary}</span> : null}
                  </div>
                ))}
              </div>
            </Panel>
          ) : null}

          <Panel title="采集进度">
            <p className="data-ops-scope-note">
              基础资料统计 {pool.toLocaleString()} 款 worker 候选；评价与在线人数只统计
              {releasedPool.toLocaleString()} 款已发售候选。已检查但 Steam 没有该字段，不再误报为 worker
              缺失。
            </p>
            <div className="bar-list">
              <Bar
                name="商店信息已检查"
                value={dimensions?.store_details_checked ?? dimensions?.store_details ?? 0}
                max={pool}
              />
              <Bar
                name="玩家评价已采集（已发售）"
                value={dimensions?.reviews_checked ?? dimensions?.reviews ?? 0}
                max={releasedPool}
              />
              <Bar
                name="在线人数已采样（已发售）"
                value={dimensions?.ccu_checked ?? dimensions?.ccu ?? 0}
                max={releasedPool}
              />
              <Bar
                name="价格状态已检查"
                value={dimensions?.price_checked ?? dimensions?.price ?? 0}
                max={pool}
              />
              <Bar name="已建检索索引" value={dimensions?.retrieval_index ?? 0} max={pool} />
              <Bar
                name="可以正常推荐"
                value={cov?.recommendation_ready_profiles ?? 0}
                max={pool}
              />
            </div>
          </Panel>

          <Panel title="可用字段量">
            <div className="section-metrics data-ops-availability">
              <div className="section-metric">
                <span>有发售日</span>
                <strong>{(dimensions?.release_date ?? 0).toLocaleString()}</strong>
                <small>款</small>
              </div>
              <div className="section-metric">
                <span>有语言信息</span>
                <strong>{(dimensions?.languages ?? 0).toLocaleString()}</strong>
                <small>款</small>
              </div>
              <div className="section-metric">
                <span>有具体价格</span>
                <strong>{(dimensions?.price ?? 0).toLocaleString()}</strong>
                <small>款</small>
              </div>
              <div className="section-metric">
                <span>有可用 CCU</span>
                <strong>{(dimensions?.ccu ?? 0).toLocaleString()}</strong>
                <small>款</small>
              </div>
              <div className="section-metric">
                <span>有封面图</span>
                <strong>{(m7?.candidates_with_cover ?? 0).toLocaleString()}</strong>
                <small>款</small>
              </div>
            </div>
          </Panel>

          {sectionStats.length > 0 ? (
            <Panel title="首页各列表体量">
              <div className="section-metrics">
                {sectionStats.map((row) => (
                  <div className="section-metric" data-tone={row.tone} key={row.name}>
                    <span>{row.name}</span>
                    <strong>{row.value.toLocaleString()}</strong>
                    <small>款</small>
                  </div>
                ))}
              </div>
            </Panel>
          ) : null}

          <Panel title="后台任务">
            <div className="task-list">
              {tasks.map((task) => {
                const run = latestRuns.find((item) => {
                  if (task.task_name === "candidate_continuation") {
                    return item.task_type === "candidate_discovery";
                  }
                  return item.task_type === task.task_name;
                });
                const running = run?.status === "running";
                const tone = task.last_error_category ? "bad" : running ? "running" : "ok";
                return (
                  <div className="task-row" data-tone={tone} key={task.task_name}>
                    <span className="task-state-dot" />
                    <div className="task-copy">
                      <strong>{taskLabel(task.task_name)}</strong>
                      <span>{running ? "正在运行" : taskStatusLine(task)}</span>
                    </div>
                    <span className="task-run-count">
                      {run ? `处理 ${run.success_count} · 请求 ${run.request_count}` : "暂无批次"}
                    </span>
                    <span className="task-next">{nextRunLabel(task.next_run_at_ms, Date.now())}</span>
                  </div>
                );
              })}
            </div>
          </Panel>
        </>
      ) : null}

      <Panel title="查一款游戏">
        <div className="lookup-row">
          <input
            value={lookup}
            onChange={(e) => setLookup(e.target.value)}
            placeholder="Steam 编号或名称，如 4108000"
            onKeyDown={(e) => {
              if (e.key === "Enter") void runLookup();
            }}
          />
          <Button onClick={() => void runLookup()}>查询</Button>
        </div>
        {lookupError ? (
          <p className="data-ops-alert" role="alert">
            {lookupError}
          </p>
        ) : null}
        {presence ? (
          <div className="presence">
            <div className="presence-steps">
              <div className="step" data-ok={presence.in_apps ? "true" : "false"}>
                <span className="step-dot" />
                {presence.in_apps ? "已进入游戏库" : "还没进游戏库（发现阶段漏了）"}
              </div>
              <div
                className="step"
                data-ok={
                  presence.has_multiplayer_profile == null
                    ? "unknown"
                    : presence.has_multiplayer_profile
                      ? "true"
                      : "false"
                }
              >
                <span className="step-dot" />
                {presence.has_multiplayer_profile == null
                  ? "是否进联机推荐池：点编号再查"
                  : presence.has_multiplayer_profile
                    ? "已在联机推荐池"
                    : "在库里，但还没进联机推荐池"}
              </div>
            </div>
            {presence.app ? (
              <div>
                <strong>{presence.app.canonical_name}</strong>
                {presence.app.release_date ? ` · 发售 ${presence.app.release_date}` : ""}
              </div>
            ) : null}
            {presence.note ? <div className="hint">{presence.note}</div> : null}
            {presence.search_hits && presence.search_hits.length > 0 ? (
              <div className="hits">
                {presence.search_hits.map((hit) => (
                  <Button
                    key={hit.app_id}
                    size="small"
                    variant="ghost"
                    onClick={() => {
                      setLookup(String(hit.app_id));
                      void (async () => {
                        if (!token) return;
                        try {
                          setPresence(await apiClient.adminAppPresence(token, hit.app_id));
                        } catch (e) {
                          setLookupError(e instanceof ApiError ? e.message : String(e));
                        }
                      })();
                    }}
                  >
                    {hit.name}
                    {hit.release_date ? ` ${hit.release_date}` : ""}
                  </Button>
                ))}
              </div>
            ) : null}
            {onOpenGame && presence.in_apps ? (
              <Button size="small" onClick={() => onOpenGame(presence.app_id)}>
                打开详情
              </Button>
            ) : null}
          </div>
        ) : null}
      </Panel>
    </section>
  );
}
