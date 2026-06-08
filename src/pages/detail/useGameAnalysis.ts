import { useEffect, useRef, useState } from "react";
import { generateGameAnalysis, getGameAnalysis } from "../../api/client";
import type { GameAnalysisReport, GameCard } from "../../types";

type AnalysisUpdatedHandler = (report: GameAnalysisReport) => Promise<void> | void;
type GameAnalysisOptions = {
  readOnly?: boolean;
  missingMessage?: string;
};

export function useGameAnalysis(
  game: GameCard,
  onAnalysisUpdated?: AnalysisUpdatedHandler,
  options: GameAnalysisOptions = {},
) {
  const [report, setReport] = useState<GameAnalysisReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  const activeRequestIdRef = useRef(0);
  const mountedRef = useRef(true);
  const currentAppidRef = useRef(game.appid);
  const onAnalysisUpdatedRef = useRef(onAnalysisUpdated);
  const optionsRef = useRef(options);

  currentAppidRef.current = game.appid;
  onAnalysisUpdatedRef.current = onAnalysisUpdated;
  optionsRef.current = options;

  useEffect(() => {
    mountedRef.current = true;

    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    setExpanded(false);
    void loadLatest(game.appid);
  }, [game.appid]);

  async function loadLatest(appid: number) {
    const requestId = beginRequest();
    setReport(null);

    try {
      const cachedReport = await getGameAnalysis(appid);
      if (!isRequestLatest(requestId, appid)) {
        return;
      }

      if (cachedReport) {
        await publishReport(cachedReport, false);
        return;
      }

      if (optionsRef.current.readOnly) {
        if (mountedRef.current) {
          setError(
            optionsRef.current.missingMessage ??
              "当前还没有可展示的分析结果。",
          );
          setLoading(false);
        }
        return;
      }

      const generatedReport = await generateGameAnalysis(appid, false);
      if (!isRequestLatest(requestId, appid)) {
        return;
      }

      if (mountedRef.current) {
        await publishReport(generatedReport, true);
      }
    } catch (nextError) {
      if (!isRequestLatest(requestId, appid)) {
        return;
      }

      if (mountedRef.current) {
        setError(getErrorMessage(nextError));
        setLoading(false);
      }
    }
  }

  async function refresh() {
    const appid = game.appid;
    if (optionsRef.current.readOnly) {
      return null;
    }

    const requestId = beginRequest();

    try {
      const generatedReport = await generateGameAnalysis(appid, true);
      if (!isRequestLatest(requestId, appid)) {
        return null;
      }

      if (mountedRef.current) {
        await publishReport(generatedReport, true);
      }
      return generatedReport;
    } catch (nextError) {
      if (!isRequestLatest(requestId, appid)) {
        return null;
      }

      if (mountedRef.current) {
        setError(getErrorMessage(nextError));
        setLoading(false);
      }
      return null;
    }
  }

  function toggleExpanded() {
    setExpanded((value) => !value);
  }

  async function publishReport(report: GameAnalysisReport, shouldNotifyParent: boolean) {
    if (mountedRef.current) {
      setReport(report);
      setLoading(false);
    }

    if (shouldNotifyParent) {
      await onAnalysisUpdatedRef.current?.(report);
    }
  }

  function beginRequest() {
    const requestId = activeRequestIdRef.current + 1;
    activeRequestIdRef.current = requestId;
    setLoading(true);
    setError(null);
    return requestId;
  }

  function isRequestLatest(requestId: number, appid: number) {
    return activeRequestIdRef.current === requestId && currentAppidRef.current === appid;
  }

  return {
    report,
    loading,
    error,
    expanded,
    refresh,
    toggleExpanded,
  };
}

function getErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : "AI 评估暂时不可用，请稍后再试。";
}
