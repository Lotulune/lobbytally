// First-launch service connection page (PRD_CS §5.1, CS-001, CS-010).
//
// The desktop client is a pure client: before any business UI appears the
// user must enter and confirm an MPGS Server address. The address is only
// persisted after the full three-step handshake (discovery -> readiness ->
// meta) succeeds. A failed first connect keeps the user on this page — no
// anonymous session is created and no online-meaning page is reachable.
// Styles: styles/screens/settings.css（.connect-screen 作用域）。

import { useRef, useState } from "react";
import {
  checkServiceConnection,
  type ConnectErrorKind,
} from "../api/discovery";
import {
  SERVICE_ORIGIN_HINT,
  SERVICE_ORIGIN_PLACEHOLDER,
  normalizeServiceOrigin,
  type OriginRejection,
} from "../api/serverOrigin";
import { Button } from "../components/Button";
import { Panel } from "../components/Panel";

const REJECTION_COPY: Record<OriginRejection, string> = {
  empty: "请输入服务地址。",
  unparseable: "地址无法解析，请检查格式（示例：https://mpgs.example.com 或 127.0.0.1:17880）。",
  unsupported_protocol:
    "域名请使用 https://；IP 地址可用 http:// 或直接填写 IP:端口。本地开发另支持 http://localhost。",
  credentials_not_allowed: "地址中不允许包含用户名或密码。",
  path_not_allowed: "地址中不允许包含路径，只需主机与端口。",
  query_not_allowed: "地址中不允许包含查询参数。",
  fragment_not_allowed: "地址中不允许包含 # 片段。",
};

/** PRD §10 error semantics -> user-facing copy. */
export function connectErrorCopy(kind: ConnectErrorKind): string {
  switch (kind) {
    case "invalid_url":
      return "地址格式不符合要求，请检查后重试。";
    case "tls_error":
      return "证书校验失败，无法建立安全连接。MPGS 不允许绕过证书错误，请确认服务端证书有效。";
    case "not_mpgs":
      return "该地址不是 MPGS Server：未找到有效的服务识别信息。";
    case "incompatible":
      return "服务协议版本不兼容，请升级客户端或更换服务。";
    case "not_ready":
      return "服务维护中，请稍后重试。";
    case "timeout":
      return "连接超时，请检查网络后重试。";
    case "network":
      return "无法连接到该服务，请检查网络连接或地址是否正确。";
  }
}

type CheckStep = "discovery" | "readiness" | "meta";

const STEP_COPY: Record<CheckStep, string> = {
  discovery: "正在识别服务…",
  readiness: "正在检查服务就绪状态…",
  meta: "正在读取服务能力…",
};

export function ConnectScreen({
  onConnected,
}: {
  /** Called with the normalized origin after the full handshake succeeds. */
  onConnected: (origin: string) => void;
}) {
  // Empty by default — never prefill a public service URL; show examples only.
  const [address, setAddress] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [step, setStep] = useState<CheckStep | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const connecting = step !== null;

  const connect = async () => {
    setError(null);
    const normalized = normalizeServiceOrigin(address, {
      allowHttpLoopback: import.meta.env.DEV,
    });
    if (!normalized.ok) {
      setError(REJECTION_COPY[normalized.reason]);
      inputRef.current?.focus();
      return;
    }

    setStep("discovery");
    const result = await checkServiceConnection(normalized.origin, {
      onStep: setStep,
    });
    setStep(null);
    if (!result.ok) {
      setError(connectErrorCopy(result.kind));
      return;
    }
    onConnected(normalized.origin);
  };

  return (
    <div className="onboarding connect-screen">
      <header className="onboarding-head">
        <h1>连接到 MPGS 服务</h1>
        <p className="sub">
          MPGS 桌面端是纯客户端，需要连接一台 MPGS Server
          才能使用推荐、搜索、社区与账户功能。你的登录凭据、缓存与待同步数据都将按服务地址隔离保存。
        </p>
      </header>

      <Panel className="connect-panel">
        <div className="stack-form">
          <label htmlFor="service-address">
            服务地址
            <input
              ref={inputRef}
              id="service-address"
              type="text"
              inputMode="url"
              autoComplete="off"
              spellCheck={false}
              placeholder={SERVICE_ORIGIN_PLACEHOLDER}
              value={address}
              disabled={connecting}
              onChange={(event) => {
                setAddress(event.target.value);
                setError(null);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !connecting) void connect();
              }}
            />
          </label>
          <p className="cal-note settings-note">{SERVICE_ORIGIN_HINT}</p>
          {error && (
            <p className="form-error connect-error" role="alert">
              {error}
            </p>
          )}
          {step && (
            <p className="cal-note settings-note connect-progress" aria-live="polite">
              <span className="spin" /> {STEP_COPY[step]}
            </p>
          )}
        </div>
      </Panel>

      <div className="onboarding-actions">
        <Button
          variant="primary"
          disabled={connecting || !address.trim()}
          onClick={() => void connect()}
        >
          {connecting ? (
            <>
              <span className="spin" /> 连接中
            </>
          ) : (
            "连接"
          )}
        </Button>
      </div>
    </div>
  );
}
