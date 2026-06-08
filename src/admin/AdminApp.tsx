import { FormEvent, useEffect, useState } from "react";
import type {
  AdminAuditEventsResponse,
  AdminDiagnosticsResponse,
  AdminOverviewResponse,
  ConfigStateResponse,
  ServiceConnectionFileResponse,
  SetupCompleteRequest,
} from "../api/generated/mpgsServerApi";
import {
  completeSetup,
  getAdminAuditEvents,
  getAdminConfigState,
  getAdminConnectionShare,
  getAdminDiagnostics,
  getAdminOverview,
  getSetupStatus,
  loginAdmin,
  requestRestart,
} from "./adminApi";
import "./AdminApp.css";

type AdminScreen =
  | { type: "loading" }
  | { type: "setup"; message?: string }
  | { type: "login"; message?: string }
  | { type: "overview" };

type AdminData = {
  overview: AdminOverviewResponse;
  diagnostics: AdminDiagnosticsResponse;
  configState: ConfigStateResponse;
  connectionShare: ServiceConnectionFileResponse;
  auditEvents: AdminAuditEventsResponse;
};

const initialSetupForm: SetupCompleteRequest = {
  setupToken: "",
  serviceName: "",
  publicBaseUrl: "",
  databaseUrl: "",
  adminToken: "",
  steamApiKey: "",
};

export function AdminApp() {
  const [screen, setScreen] = useState<AdminScreen>({ type: "loading" });
  const [setupForm, setSetupForm] =
    useState<SetupCompleteRequest>(initialSetupForm);
  const [adminToken, setAdminToken] = useState("");
  const [adminData, setAdminData] = useState<AdminData | null>(null);
  const [isBusy, setIsBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [restartMessage, setRestartMessage] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;

    void getSetupStatus()
      .then((status) => {
        if (disposed) {
          return;
        }
        setScreen(status.configured ? { type: "login" } : { type: "setup" });
      })
      .catch((nextError) => {
        if (disposed) {
          return;
        }
        setError(errorMessage(nextError));
        setScreen({ type: "login" });
      });

    return () => {
      disposed = true;
    };
  }, []);

  async function handleSetupSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setIsBusy(true);
    setError(null);

    try {
      await completeSetup(setupForm);
      setSetupForm(initialSetupForm);
      setScreen({ type: "setup", message: "配置已写入，重启后生效。" });
    } catch (nextError) {
      setError(errorMessage(nextError));
    } finally {
      setIsBusy(false);
    }
  }

  async function handleLoginSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setIsBusy(true);
    setError(null);

    try {
      await loginAdmin(adminToken);
      setAdminToken("");
      await refreshAdminData();
      setScreen({ type: "overview" });
    } catch (nextError) {
      setError(errorMessage(nextError));
    } finally {
      setIsBusy(false);
    }
  }

  async function refreshAdminData() {
    const [overview, diagnostics, configState, connectionShare, auditEvents] =
      await Promise.all([
        getAdminOverview(),
        getAdminDiagnostics(),
        getAdminConfigState(),
        getAdminConnectionShare(),
        getAdminAuditEvents(),
      ]);
    setAdminData({ overview, diagnostics, configState, connectionShare, auditEvents });
  }

  async function handleRefresh() {
    setIsBusy(true);
    setError(null);
    try {
      await refreshAdminData();
    } catch (nextError) {
      setError(errorMessage(nextError));
    } finally {
      setIsBusy(false);
    }
  }

  async function handleRestart() {
    setIsBusy(true);
    setError(null);
    setRestartMessage(null);
    try {
      const response = await requestRestart();
      setRestartMessage(
        response.restartScheduled
          ? "重启已请求，等待 Compose 拉起新进程。"
          : "重启未调度。",
      );
    } catch (nextError) {
      setError(errorMessage(nextError));
    } finally {
      setIsBusy(false);
    }
  }

  return (
    <main className="admin-shell">
      <aside className="admin-sidebar">
        <div className="admin-brand">
          <span className="admin-mark" aria-hidden="true">
            M
          </span>
          <div>
            <strong>MPGS</strong>
            <span>公共发现服务</span>
          </div>
        </div>
        <nav aria-label="管理导航" className="admin-nav">
          <span className={screen.type === "overview" ? "active" : ""}>
            管理概览
          </span>
          <span>首次配置</span>
          <span>部署诊断</span>
          <span>连接分享</span>
        </nav>
      </aside>

      <section className="admin-surface">
        {screen.type === "loading" && (
          <div className="admin-panel admin-loading">正在读取服务状态...</div>
        )}

        {screen.type === "setup" && (
          <SetupPanel
            form={setupForm}
            isBusy={isBusy}
            message={screen.message}
            onChange={setSetupForm}
            onSubmit={handleSetupSubmit}
          />
        )}

        {screen.type === "login" && (
          <LoginPanel
            adminToken={adminToken}
            isBusy={isBusy}
            message={screen.message}
            onAdminTokenChange={setAdminToken}
            onSubmit={handleLoginSubmit}
          />
        )}

        {screen.type === "overview" && adminData && (
          <OverviewPanel
            data={adminData}
            isBusy={isBusy}
            restartMessage={restartMessage}
            onRefresh={handleRefresh}
            onRestart={handleRestart}
          />
        )}

        {error && <p className="admin-error">{error}</p>}
      </section>
    </main>
  );
}

function SetupPanel({
  form,
  isBusy,
  message,
  onChange,
  onSubmit,
}: {
  form: SetupCompleteRequest;
  isBusy: boolean;
  message?: string;
  onChange: (form: SetupCompleteRequest) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
}) {
  return (
    <form className="admin-panel admin-form" onSubmit={onSubmit}>
      <header className="admin-section-head">
        <div>
          <span>Setup</span>
          <h1>首次配置</h1>
        </div>
      </header>

      <div className="admin-form-grid">
        <AdminInput
          label="引导令牌"
          type="password"
          value={form.setupToken}
          onChange={(value) => onChange({ ...form, setupToken: value })}
        />
        <AdminInput
          label="服务名称"
          value={form.serviceName}
          onChange={(value) => onChange({ ...form, serviceName: value })}
        />
        <AdminInput
          label="公开服务地址"
          value={form.publicBaseUrl}
          onChange={(value) => onChange({ ...form, publicBaseUrl: value })}
        />
        <AdminInput
          label="Postgres 地址"
          value={form.databaseUrl}
          onChange={(value) => onChange({ ...form, databaseUrl: value })}
        />
        <AdminInput
          label="管理员令牌"
          type="password"
          value={form.adminToken}
          onChange={(value) => onChange({ ...form, adminToken: value })}
        />
        <AdminInput
          label="Steam API Key"
          type="password"
          value={form.steamApiKey}
          onChange={(value) => onChange({ ...form, steamApiKey: value })}
        />
      </div>

      <div className="admin-actions">
        <button className="admin-primary" disabled={isBusy} type="submit">
          完成配置
        </button>
        {message && <span className="admin-success">{message}</span>}
      </div>
    </form>
  );
}

function LoginPanel({
  adminToken,
  isBusy,
  message,
  onAdminTokenChange,
  onSubmit,
}: {
  adminToken: string;
  isBusy: boolean;
  message?: string;
  onAdminTokenChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
}) {
  return (
    <form className="admin-panel admin-login" onSubmit={onSubmit}>
      <header className="admin-section-head">
        <div>
          <span>Admin</span>
          <h1>管理员登录</h1>
        </div>
      </header>

      <AdminInput
        label="管理员令牌"
        type="password"
        value={adminToken}
        onChange={onAdminTokenChange}
      />

      <div className="admin-actions">
        <button className="admin-primary" disabled={isBusy} type="submit">
          登录
        </button>
        {message && <span className="admin-success">{message}</span>}
      </div>
    </form>
  );
}

function OverviewPanel({
  data,
  isBusy,
  restartMessage,
  onRefresh,
  onRestart,
}: {
  data: AdminData;
  isBusy: boolean;
  restartMessage: string | null;
  onRefresh: () => void;
  onRestart: () => void;
}) {
  const [shareMessage, setShareMessage] = useState<string | null>(null);

  async function handleCopyServiceAddress() {
    try {
      await navigator.clipboard.writeText(data.connectionShare.baseUrl);
      setShareMessage("服务地址已复制。");
    } catch {
      setShareMessage("复制失败，请手动复制服务地址。");
    }
  }

  function handleDownloadConnectionFile() {
    const blob = new Blob(
      [JSON.stringify(data.connectionShare, null, 2)],
      { type: "application/json" },
    );
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "mpgs-service-connection.json";
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    URL.revokeObjectURL(url);
    setShareMessage("连接文件已下载。");
  }

  return (
    <div className="admin-overview">
      <header className="admin-section-head">
        <div>
          <span>Overview</span>
          <h1>管理概览</h1>
        </div>
        <div className="admin-actions">
          <button className="admin-secondary" disabled={isBusy} onClick={onRefresh} type="button">
            刷新
          </button>
          <button className="admin-primary" disabled={isBusy} onClick={onRestart} type="button">
            请求重启
          </button>
        </div>
      </header>

      <section className="admin-metrics" aria-label="服务摘要">
        <Metric label="服务名称" value={data.overview.serviceName} />
        <Metric
          label="公共库状态"
          value={catalogStatusLabel(data.overview.publicCatalogStatus)}
        />
        <Metric label="公开游戏" value={String(data.overview.publicGameCount)} />
        <Metric label="待审核" value={String(data.overview.pendingReviewCount)} />
      </section>

      <section className="admin-grid">
        <div className="admin-panel">
          <h2>部署诊断</h2>
          <DefinitionList
            items={[
              ["Postgres", data.diagnostics.postgres],
              ["Active config", data.diagnostics.activeConfig],
              ["HTTPS", data.diagnostics.httpsStatus],
              ["Public CORS", data.diagnostics.publicCors],
              ["Steam", data.diagnostics.steam],
              ["LLM", data.diagnostics.llm],
              ["R2", data.diagnostics.r2],
            ]}
          />
        </div>

        <div className="admin-panel">
          <h2>配置状态</h2>
          <DefinitionList
            items={[
              ["Active", data.configState.activeConfigVersion],
              ["Pending", data.configState.pendingConfigVersion ?? "无"],
              ["Restart", data.configState.restartRequired ? "需要重启" : "无需重启"],
              ["Startup", data.configState.lastStartupStatus],
            ]}
          />
        </div>

        <div className="admin-panel">
          <h2>连接分享</h2>
          <DefinitionList
            items={[
              ["Base URL", data.connectionShare.baseUrl],
              ["Service info", data.connectionShare.serviceInfoUrl],
              ["API", data.connectionShare.apiVersion],
              ["Instance", data.connectionShare.serviceInstanceId],
            ]}
          />
          <div className="admin-actions admin-panel-actions">
            <button
              className="admin-secondary"
              onClick={handleCopyServiceAddress}
              type="button"
            >
              复制服务地址
            </button>
            <button
              className="admin-primary"
              onClick={handleDownloadConnectionFile}
              type="button"
            >
              下载连接文件
            </button>
          </div>
          {shareMessage && <p className="admin-success block">{shareMessage}</p>}
        </div>

        <div className="admin-panel">
          <h2>运行状态</h2>
          <DefinitionList
            items={[
              ["Safe mode", data.diagnostics.safeMode ? "开启" : "关闭"],
              ["Restart required", data.overview.restartRequired ? "是" : "否"],
              [
                "Connection share",
                data.overview.connectionShareConfigured ? "已配置" : "未配置",
              ],
              ["Restart policy", data.diagnostics.restartPolicy],
            ]}
          />
          {restartMessage && <p className="admin-success block">{restartMessage}</p>}
        </div>

        <div className="admin-panel">
          <h2>最近审计</h2>
          {data.overview.latestAuditEvent ? (
            <DefinitionList
              items={[
                ["Event", data.overview.latestAuditEvent.eventType],
                ["Actor", data.overview.latestAuditEvent.actor],
                ["Outcome", data.overview.latestAuditEvent.outcome],
              ]}
            />
          ) : (
            <p className="admin-muted">暂无审计事件</p>
          )}
        </div>

        <div className="admin-panel">
          <h2>运维日志</h2>
          {data.auditEvents.events.length > 0 ? (
            <div className="admin-log-list">
              {data.auditEvents.events.map((event, index) => (
                <div
                  className="admin-log-row"
                  key={`${event.eventType}-${event.actor}-${event.outcome}-${index}`}
                >
                  <strong>{event.eventType}</strong>
                  <span>{event.actor}</span>
                  <em>{event.outcome}</em>
                </div>
              ))}
            </div>
          ) : (
            <p className="admin-muted">暂无运维日志</p>
          )}
        </div>
      </section>
    </div>
  );
}

function AdminInput({
  label,
  value,
  onChange,
  type = "text",
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  type?: "text" | "password";
}) {
  return (
    <label className="admin-field">
      <span>{label}</span>
      <input
        autoComplete="off"
        type={type}
        value={value}
        onChange={(event) => onChange(event.currentTarget.value)}
      />
    </label>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="admin-metric">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function DefinitionList({ items }: { items: Array<[string, string]> }) {
  return (
    <dl className="admin-dl">
      {items.map(([label, value]) => (
        <div key={label}>
          <dt>{label}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function catalogStatusLabel(status: AdminOverviewResponse["publicCatalogStatus"]) {
  switch (status) {
    case "empty":
      return "公共库为空";
    case "ready":
      return "公共库就绪";
    case "updating":
      return "正在更新";
    case "unavailable":
      return "暂不可用";
    default:
      return status;
  }
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
