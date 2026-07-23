// Settings > 服务 panel (PRD_CS CS-009, §5.3).
//
// Shows the active service and link health, tests a candidate address with
// the full connection handshake, switches services only after explicit user
// confirmation, and deletes a service's local data on request. The old
// service's data is never touched by a switch; the old token is never sent
// to the new service because switching reloads into a fresh origin-scoped
// module graph.
// Styles: styles/screens/settings.css（.settings-screen 作用域）。

import { useEffect, useState } from "react";
import { checkServiceConnection } from "../api/discovery";
import { activateServiceOrigin, normalizeServiceOrigin } from "../api/serverOrigin";
import { getClientStorage } from "../api/storage";
import { getConnectionManager, type ConnectionSnapshot } from "../app/connection";
import { useToast } from "../app/ToastProvider";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";
import { Modal } from "../components/Modal";
import { Panel } from "../components/Panel";
import { connectErrorCopy } from "./ConnectScreen";

function statusChip(snapshot: ConnectionSnapshot | null) {
  if (!snapshot) return null;
  switch (snapshot.status) {
    case "connected":
      return <Chip tone="ok">已连接</Chip>;
    case "checking":
      return <Chip>检查中</Chip>;
    case "maintenance":
      return <Chip tone="warn">服务维护中</Chip>;
    case "offline":
      return <Chip tone="danger">离线</Chip>;
  }
}

function formatMs(ms: number): string {
  return new Date(ms).toLocaleString();
}

export function ServicePanel() {
  const toast = useToast();
  const manager = getConnectionManager();
  const [snapshot, setSnapshot] = useState<ConnectionSnapshot>(() => manager.get());
  const [known, setKnown] = useState(() => manager.knownServices());
  const [candidate, setCandidate] = useState("");
  const [testing, setTesting] = useState(false);
  const [testError, setTestError] = useState<string | null>(null);
  const [testedOrigin, setTestedOrigin] = useState<string | null>(null);
  const [confirmSwitch, setConfirmSwitch] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [offlineSwitchOrigin, setOfflineSwitchOrigin] = useState<string | null>(null);

  useEffect(
    () =>
      manager.subscribe((next) => {
        setSnapshot(next);
        setKnown(manager.knownServices());
      }),
    [manager],
  );

  /** Switch = activate + reload; the rebuild binds every singleton to the
   *  new origin namespace and stops all old-origin requests (PRD §9). */
  const switchTo = (origin: string) => {
    activateServiceOrigin(getClientStorage(), origin);
    window.location.reload();
  };

  const test = async () => {
    setTestError(null);
    setTestedOrigin(null);
    setOfflineSwitchOrigin(null);
    const normalized = normalizeServiceOrigin(candidate, {
      allowHttpLoopback: import.meta.env.DEV,
    });
    if (!normalized.ok) {
      setTestError(
        "地址格式不符合要求：可用 https://主机[:端口]、IP:端口 或 http://IP[:端口]；勿含路径、查询或用户信息。",
      );
      return;
    }
    if (normalized.origin === snapshot.origin) {
      setTestError("该地址是当前正在使用的服务。");
      return;
    }
    setTesting(true);
    try {
      const result = await checkServiceConnection(normalized.origin);
      if (result.ok) {
        setTestedOrigin(normalized.origin);
      } else {
        setTestError(connectErrorCopy(result.kind));
        // A previously-known service may still be switched to offline so the
        // user can reach its local cache (PRD §5.3 step 5).
        if (known.some((entry) => entry.origin === normalized.origin)) {
          setOfflineSwitchOrigin(normalized.origin);
        }
      }
    } finally {
      setTesting(false);
    }
  };

  const removeService = (origin: string) => {
    const { removedKeys, wasCurrent } = manager.deleteServiceData(origin);
    setConfirmDelete(null);
    setKnown(manager.knownServices());
    if (wasCurrent) {
      // No active service remains: reload into the connect flow.
      window.location.reload();
      return;
    }
    toast.show(`已删除该服务的 ${removedKeys} 项本地数据`);
  };

  return (
    <Panel title="服务" aria-label="服务">
      <div className="service-current">
        <div className="statusline">
          {statusChip(snapshot)}
          {snapshot.origin && <span className="service-origin">{snapshot.origin}</span>}
        </div>
        {snapshot.lastError && snapshot.status !== "connected" && (
          <p className="cal-note settings-note">{connectErrorCopy(snapshot.lastError)}</p>
        )}
        <div className="seg">
          <Button size="small" onClick={() => void manager.recheck()}>
            重新检查连接
          </Button>
        </div>
      </div>

      <h5 className="pref-group-title">更换服务</h5>
      <div className="stack-form">
        <label htmlFor="service-candidate">
          新服务地址
          <input
            id="service-candidate"
            type="url"
            inputMode="url"
            autoComplete="off"
            spellCheck={false}
            placeholder="https://mpgs.example.com"
            value={candidate}
            disabled={testing}
            onChange={(event) => {
              setCandidate(event.target.value);
              setTestError(null);
              setTestedOrigin(null);
              setOfflineSwitchOrigin(null);
            }}
          />
        </label>
        {testError && (
          <p className="form-error" role="alert">
            {testError}
          </p>
        )}
        {testedOrigin && (
          <p className="cal-note settings-note">
            检查通过：{testedOrigin} 是一台兼容的 MPGS Server。
          </p>
        )}
        <div className="settings-actions">
          <Button
            size="small"
            disabled={testing || !candidate.trim()}
            onClick={() => void test()}
          >
            {testing ? (
              <>
                <span className="spin" /> 测试中
              </>
            ) : (
              "测试连接"
            )}
          </Button>
          {testedOrigin && (
            <Button
              size="small"
              variant="primary"
              onClick={() => setConfirmSwitch(testedOrigin)}
            >
              切换到此服务
            </Button>
          )}
          {offlineSwitchOrigin && (
            <Button
              size="small"
              onClick={() => setConfirmSwitch(offlineSwitchOrigin)}
            >
              仍然切换（离线查看该服务缓存）
            </Button>
          )}
        </div>
        <p className="cal-note settings-note">
          切换前会再次确认；旧服务的登录与缓存会完整保留在本机，可随时切回。
        </p>
      </div>

      {known.length > 0 && (
        <>
          <h5 className="pref-group-title">已知服务</h5>
          <ul className="service-known-list">
            {known.map((entry) => (
              <li
                key={entry.origin}
                className={`service-known-item${entry.origin === snapshot.origin ? " current" : ""}`}
              >
                <span className="service-origin">
                  {entry.origin}
                  {entry.origin === snapshot.origin && "（当前）"}
                  <br />
                  <small>最近连接：{formatMs(entry.lastConnectedAtMs)}</small>
                </span>
                <span className="service-known-actions">
                  {entry.origin !== snapshot.origin && (
                    <Button size="small" onClick={() => setConfirmSwitch(entry.origin)}>
                      切换
                    </Button>
                  )}
                  <Button size="small" onClick={() => setConfirmDelete(entry.origin)}>
                    删除本地数据
                  </Button>
                </span>
              </li>
            ))}
          </ul>
        </>
      )}

      {confirmSwitch && (
        <Modal
          title="切换服务"
          titleId="switch-service-title"
          onClose={() => setConfirmSwitch(null)}
        >
          <p>
            确定切换到 <strong className="service-origin">{confirmSwitch}</strong> 吗？
          </p>
          <p className="cal-note settings-note">
            当前服务的登录令牌不会发送给新服务；两个服务的缓存与待同步数据分别隔离保存。
          </p>
          <div className="settings-actions">
            <Button variant="ghost" onClick={() => setConfirmSwitch(null)}>
              取消
            </Button>
            <Button variant="primary" onClick={() => switchTo(confirmSwitch)}>
              确认切换
            </Button>
          </div>
        </Modal>
      )}

      {confirmDelete && (
        <Modal
          title="删除本地数据"
          titleId="delete-service-title"
          onClose={() => setConfirmDelete(null)}
        >
          <p>
            确定删除 <strong className="service-origin">{confirmDelete}</strong> 的全部本地数据吗？
          </p>
          <p className="cal-note settings-note">
            包括该服务的登录会话、缓存快照与未同步的反馈/偏好。此操作不可撤销，服务端数据不受影响。
          </p>
          <div className="settings-actions">
            <Button variant="ghost" onClick={() => setConfirmDelete(null)}>
              取消
            </Button>
            <Button variant="primary" onClick={() => removeService(confirmDelete)}>
              确认删除
            </Button>
          </div>
        </Modal>
      )}
    </Panel>
  );
}
