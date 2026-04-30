import type { DashboardPayload } from "../../types";

export function AboutPage({
  config,
  stats,
}: {
  config: DashboardPayload["config"];
  stats: DashboardPayload["stats"];
}) {
  return (
    <section className="about-page">
      <h2>关于 Co-Play</h2>
      <p className="about-copy">
        Co-Play 是一个围绕 Steam 多人游戏发现、筛选和轻量 AI 辅助的本地桌面应用。
      </p>

      <div className="about-grid">
        <article className="about-card">
          <strong>当前数据规模</strong>
          <p>库内 {formatNumber(stats.totalGames)} 款游戏，新游区 {formatNumber(stats.newGamesCount)} 款。</p>
          <p>精品老游区 {formatNumber(stats.classicGamesCount)} 款，种子数 {formatNumber(stats.seedCount)}。</p>
        </article>

        <article className="about-card">
          <strong>运行配置</strong>
          <p>Steam Key：{config.steamApiKeyConfigured ? "已配置" : "未配置"}</p>
          <p>LLM Key：{config.llmApiKeyConfigured ? "已配置" : "未配置"}</p>
          <p>地区 / 语言：{config.country} / {config.language}</p>
        </article>

        <article className="about-card">
          <strong>同步与诊断</strong>
          <p>最近同步：{formatDateTime(stats.lastSyncAt)}</p>
          <p>最近处理 AppID：{stats.lastDiscoveryAppid ?? "无"}</p>
          <p>数据来源：{stats.dataSource}</p>
        </article>
      </div>
    </section>
  );
}

function formatNumber(value?: number | null) {
  return typeof value === "number" ? value.toLocaleString("zh-CN") : "—";
}

function formatDateTime(value?: string | null) {
  if (!value) return "未同步";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}
