import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "./api";
import { createTranslator, getLocale } from "./i18n";
import type {
  InstallationInfo,
  InstallRequest,
  InstallerError,
  OperationProgress,
  PreflightReport,
  ReleaseInfo,
} from "./types";
import { compareVersions } from "./version";

const EULA_URL = "https://github.com/aseprite/aseprite/blob/main/EULA.txt";
const ASEPRITE_URL = "https://www.aseprite.org/";
const PROJECT_URL = "https://github.com/fmhun/asprite-installer";

type View = "status" | "release" | "preflight" | "install";

const initialProgress: OperationProgress = {
  stage: "idle",
  percent: null,
  message: "",
  logLine: null,
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = units[0];
  for (let index = 1; value >= 1024 && index < units.length; index += 1) {
    value /= 1024;
    unit = units[index];
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${unit}`;
}

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as InstallerError).message);
  }
  return String(error);
}

function installationPriority(installation: InstallationInfo): number {
  return { managed: 0, manual: 1, steam: 2, packageManager: 3 }[
    installation.channel
  ];
}

function App() {
  const t = useMemo(() => createTranslator(getLocale()), []);
  const [view, setView] = useState<View>("status");
  const [installations, setInstallations] = useState<InstallationInfo[]>([]);
  const [releases, setReleases] = useState<ReleaseInfo[]>([]);
  const [preflight, setPreflight] = useState<PreflightReport | null>(null);
  const [flowTarget, setFlowTarget] = useState<InstallationInfo | null>(null);
  const [completedInstallation, setCompletedInstallation] =
    useState<InstallationInfo | null>(null);
  const [includePrereleases, setIncludePrereleases] = useState(false);
  const [selectedTag, setSelectedTag] = useState("");
  const [loading, setLoading] = useState(true);
  const [releaseLoading, setReleaseLoading] = useState(false);
  const [preflightLoading, setPreflightLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [installingTools, setInstallingTools] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [progress, setProgress] = useState<OperationProgress>(initialProgress);
  const [logs, setLogs] = useState<string[]>([]);
  const [showEula, setShowEula] = useState(false);
  const [eulaAccepted, setEulaAccepted] = useState(false);

  const refreshInstallations = useCallback(async (showSpinner = true) => {
    if (showSpinner) setLoading(true);
    setError(null);
    try {
      setInstallations(await api.scanInstallations());
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refreshInstallations();
  }, [refreshInstallations]);

  const loadReleases = useCallback(async () => {
    setReleaseLoading(true);
    setError(null);
    try {
      const releaseData = await api.listReleases(includePrereleases);
      setReleases(releaseData);
      setSelectedTag((current) => {
        if (releaseData.some((release) => release.tag === current)) {
          return current;
        }
        return (
          releaseData.find((release) => release.latest)?.tag ??
          releaseData[0]?.tag ??
          ""
        );
      });
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setReleaseLoading(false);
    }
  }, [includePrereleases]);

  useEffect(() => {
    if (view === "release") void loadReleases();
  }, [loadReleases, view]);

  const sortedInstallations = useMemo(
    () => [...installations].sort((a, b) => installationPriority(a) - installationPriority(b)),
    [installations],
  );
  const primaryInstallation = sortedInstallations[0] ?? null;
  const otherInstallations = sortedInstallations.slice(1);
  const selectedRelease = releases.find((release) => release.tag === selectedTag);

  const actionLabel = useMemo(() => {
    if (!flowTarget || flowTarget.channel === "manual") return t("install");
    const comparison = compareVersions(selectedTag, flowTarget.version);
    if (comparison > 0) return t("update");
    if (comparison < 0) return t("downgrade");
    return t("reinstall");
  }, [flowTarget, selectedTag, t]);

  const startFlow = (target: InstallationInfo | null) => {
    setFlowTarget(target);
    setCompletedInstallation(null);
    setPreflight(null);
    setError(null);
    setNotice(null);
    setView("release");
  };

  const openPreflight = async () => {
    setView("preflight");
    setPreflightLoading(true);
    setError(null);
    try {
      setPreflight(await api.runPreflight());
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setPreflightLoading(false);
    }
  };

  const startInstall = async () => {
    if (!selectedRelease || !eulaAccepted) return;
    const request: InstallRequest = {
      tag: selectedRelease.tag,
      targetPath:
        flowTarget?.channel === "manual" || flowTarget?.channel === "managed"
          ? flowTarget.path
          : null,
      adopt: flowTarget?.channel === "manual",
      eulaAccepted,
    };
    setShowEula(false);
    setView("install");
    setBusy(true);
    setError(null);
    setNotice(null);
    setLogs([]);
    setProgress({
      stage: "preflight",
      percent: 0,
      message: t("checking"),
      logLine: null,
    });
    try {
      const installed = await api.startInstall(request, (event) => {
        setProgress(event);
        if (event.logLine) {
          setLogs((current) => [...current.slice(-499), event.logLine!]);
        }
      });
      setCompletedInstallation(installed);
      setProgress((current) => ({
        ...current,
        stage: "completed",
        percent: 100,
        message: t("installComplete"),
      }));
      await refreshInstallations(false);
    } catch (caught) {
      setError(errorMessage(caught));
      setProgress((current) => ({ ...current, stage: "failed" }));
    } finally {
      setBusy(false);
    }
  };

  const installTools = async () => {
    setInstallingTools(true);
    setError(null);
    try {
      setPreflight(await api.installBuildTools());
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setInstallingTools(false);
    }
  };

  const restore = async (installation: InstallationInfo) => {
    if (!window.confirm(t("confirmRestore"))) return;
    setBusy(true);
    setError(null);
    try {
      await api.restorePrevious(installation.id);
      setNotice(t("restoreComplete"));
      await refreshInstallations(false);
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  };

  const uninstall = async (installation: InstallationInfo) => {
    if (!window.confirm(t("confirmUninstall"))) return;
    setBusy(true);
    setError(null);
    try {
      await api.uninstallManaged(installation.id);
      setNotice(t("uninstallComplete"));
      await refreshInstallations(false);
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  };

  const cleanCache = async () => {
    setError(null);
    try {
      const size = await api.cleanCache();
      setNotice(t("cacheCleaned", { size: formatBytes(size) }));
    } catch (caught) {
      setError(errorMessage(caught));
    }
  };

  const returnToStatus = () => {
    setView("status");
    setFlowTarget(null);
    setCompletedInstallation(null);
    setProgress(initialProgress);
    setLogs([]);
    setError(null);
  };

  if (loading) {
    return (
      <main className="loading-screen">
        <img src="/icon.png" alt="" />
        <div className="spinner" aria-hidden="true" />
        <p>{t("checkingInstallation")}</p>
      </main>
    );
  }

  return (
    <main className="app-shell">
      <header className="app-header compact-header">
        <div className="brand">
          <img className="app-icon" src="/icon.png" alt="" />
          <div>
            <div className="eyebrow">{t("unofficial")}</div>
            <h1>Aseprite Installer</h1>
          </div>
        </div>
      </header>

      {view !== "status" && view !== "install" && (
        <nav className="stepper" aria-label={t("installationSteps")}>
          {(["release", "preflight", "install"] as const).map((step, index) => {
            const currentIndex = ["release", "preflight", "install"].indexOf(view);
            return (
              <div
                className={`${view === step ? "current" : ""} ${index < currentIndex ? "done" : ""}`}
                key={step}
              >
                <span>{index + 1}</span>
                <small>{t(step === "release" ? "releaseStep" : step === "preflight" ? "toolsStep" : "installStep")}</small>
              </div>
            );
          })}
        </nav>
      )}

      {error && (
        <div className="alert error" role="alert">
          <strong>{t("operationFailed")}</strong>
          <span>{error}</span>
          <button aria-label={t("close")} onClick={() => setError(null)}>×</button>
        </div>
      )}
      {notice && (
        <div className="alert success" role="status">
          <span>{notice}</span>
          <button aria-label={t("close")} onClick={() => setNotice(null)}>×</button>
        </div>
      )}

      {view === "status" && (
        <section className="context-card status-view">
          {primaryInstallation ? (
            <>
              <div className="status-symbol success-symbol" aria-hidden="true">✓</div>
              <div className="status-copy">
                <span className={`channel ${primaryInstallation.channel}`}>
                  {t(primaryInstallation.channel)}
                </span>
                <h2>{t("alreadyInstalled")}</h2>
                <p className="version-line">
                  Aseprite {primaryInstallation.version?.replace(/^v/, "") ?? t("unknownVersion")}
                  {primaryInstallation.architecture ? ` · ${primaryInstallation.architecture}` : ""}
                </p>
                <p className="path" title={primaryInstallation.path}>{primaryInstallation.path}</p>
                {primaryInstallation.channel === "manual" && (
                  <p className="context-note">{t("manualStatusHint")}</p>
                )}
                {(primaryInstallation.channel === "steam" ||
                  primaryInstallation.channel === "packageManager") && (
                  <p className="context-note">{t("externalReadOnly")}</p>
                )}
              </div>
              <div className="primary-actions">
                <button
                  className="button primary full"
                  onClick={() => void api.launchInstallation(primaryInstallation.id)}
                >
                  ▶ {t("openAseprite")}
                </button>
                {primaryInstallation.channel === "managed" || primaryInstallation.channel === "manual" ? (
                  <button
                    className="button secondary full"
                    disabled={busy}
                    onClick={() => startFlow(primaryInstallation)}
                  >
                    {primaryInstallation.channel === "manual" ? t("manageInstallation") : t("changeVersion")}
                  </button>
                ) : (
                  <button className="button secondary full" onClick={() => startFlow(null)}>
                    {t("installSeparateCopy")}
                  </button>
                )}
              </div>
              <details className="more-options">
                <summary>{t("moreOptions")}</summary>
                <div>
                  <button className="button ghost compact" onClick={() => void api.revealInstallation(primaryInstallation.id)}>
                    {t("reveal")}
                  </button>
                  {primaryInstallation.channel === "managed" && primaryInstallation.hasBackup && (
                    <button className="button ghost compact" disabled={busy} onClick={() => void restore(primaryInstallation)}>
                      {t("restore")}
                    </button>
                  )}
                  {primaryInstallation.channel === "managed" && (
                    <button className="button danger ghost compact" disabled={busy} onClick={() => void uninstall(primaryInstallation)}>
                      {t("uninstall")}
                    </button>
                  )}
                  <button className="button ghost compact" onClick={() => void cleanCache()}>{t("cleanCache")}</button>
                  <button className="button ghost compact" disabled={busy} onClick={() => void refreshInstallations()}>{t("refresh")}</button>
                  <button className="button ghost compact" onClick={() => void api.openExternal(ASEPRITE_URL)}>Aseprite ↗</button>
                  <button className="button ghost compact" onClick={() => void api.openExternal(PROJECT_URL)}>GitHub ↗</button>
                </div>
              </details>
              {otherInstallations.length > 0 && (
                <details className="other-installations">
                  <summary>{t("otherInstallations", { count: String(otherInstallations.length) })}</summary>
                  {otherInstallations.map((installation) => (
                    <div className="other-installation" key={installation.id}>
                      <span>{t(installation.channel)} · Aseprite {installation.version?.replace(/^v/, "") ?? "?"}</span>
                      <button onClick={() => void api.launchInstallation(installation.id)}>{t("open")}</button>
                    </div>
                  ))}
                </details>
              )}
            </>
          ) : (
            <>
              <div className="status-symbol" aria-hidden="true">+</div>
              <div className="status-copy">
                <h2>{t("notInstalled")}</h2>
                <p>{t("notInstalledHint")}</p>
              </div>
              <div className="primary-actions">
                <button className="button primary full" onClick={() => startFlow(null)}>
                  {t("installAseprite")} →
                </button>
              </div>
              <details className="more-options">
                <summary>{t("moreOptions")}</summary>
                <div>
                  <button className="button ghost compact" onClick={() => void cleanCache()}>{t("cleanCache")}</button>
                  <button className="button ghost compact" onClick={() => void refreshInstallations()}>{t("refresh")}</button>
                  <button className="button ghost compact" onClick={() => void api.openExternal(ASEPRITE_URL)}>Aseprite ↗</button>
                  <button className="button ghost compact" onClick={() => void api.openExternal(PROJECT_URL)}>GitHub ↗</button>
                </div>
              </details>
            </>
          )}
        </section>
      )}

      {view === "release" && (
        <section className="context-card flow-view">
          <div className="flow-heading">
            <button className="back-button" onClick={returnToStatus}>← {t("back")}</button>
            <span className="step-label">{t("stepOf", { current: "1", total: "3" })}</span>
            <h2>{t("chooseVersionTitle")}</h2>
            <p>{t("chooseVersionBody")}</p>
          </div>
          {releaseLoading && releases.length === 0 ? (
            <div className="inline-loading"><div className="spinner" />{t("loadingReleases")}</div>
          ) : (
            <>
              <label className="field-label" htmlFor="release">{t("selectRelease")}</label>
              <select id="release" value={selectedTag} onChange={(event) => setSelectedTag(event.target.value)}>
                {releases.map((release) => (
                  <option key={release.tag} value={release.tag}>
                    {release.tag.replace(/^v/, "")}
                    {release.latest ? ` — ${t("latest")}` : ""}
                    {release.prerelease ? ` — ${t("beta")}` : ""}
                  </option>
                ))}
              </select>
              {selectedRelease && (
                <div className="release-summary">
                  <span>{new Date(selectedRelease.publishedAt).toLocaleDateString()}</span>
                  <span>{formatBytes(selectedRelease.size)}</span>
                  <span className="checksum">SHA-256 ✓</span>
                </div>
              )}
              <label className="switch-row compact-switch">
                <input type="checkbox" checked={includePrereleases} onChange={(event) => setIncludePrereleases(event.target.checked)} />
                <span>{t("includePrereleases")}</span>
              </label>
              <button className="button primary full next-button" disabled={!selectedRelease || releaseLoading} onClick={() => void openPreflight()}>
                {t("continueToChecks")} →
              </button>
            </>
          )}
        </section>
      )}

      {view === "preflight" && (
        <section className="context-card flow-view">
          <div className="flow-heading">
            <button className="back-button" onClick={() => setView("release")}>← {t("back")}</button>
            <span className="step-label">{t("stepOf", { current: "2", total: "3" })}</span>
            <h2>{t("checkToolsTitle")}</h2>
            <p>{t("checkToolsBody")}</p>
          </div>
          {preflightLoading ? (
            <div className="inline-loading"><div className="spinner" />{t("checkingTools")}</div>
          ) : preflight ? (
            <>
              <ul className="check-list single-column">
                {preflight.prerequisites.map((item) => (
                  <li className={item.ok ? "ok" : "missing"} key={item.id}>
                    <span className="check-mark">{item.ok ? "✓" : "!"}</span>
                    <div><strong>{item.label}</strong><small>{item.detail}</small></div>
                  </li>
                ))}
              </ul>
              {preflight.homebrewAvailable && preflight.prerequisites.some((item) => !item.ok && (item.id === "cmake" || item.id === "ninja")) && (
                <button className="button secondary full" disabled={installingTools} onClick={() => void installTools()}>
                  {installingTools ? t("installingTools") : t("installTools")}
                </button>
              )}
              {!preflight.ready && <p className="blocking-note">{t("fixRequirements")}</p>}
              <button
                className="button primary full next-button"
                disabled={!preflight.ready || installingTools}
                onClick={() => { setEulaAccepted(false); setShowEula(true); }}
              >
                {actionLabel} →
              </button>
            </>
          ) : null}
        </section>
      )}

      {view === "install" && (
        <section className="context-card progress-view">
          {completedInstallation ? (
            <>
              <div className="status-symbol success-symbol" aria-hidden="true">✓</div>
              <div className="status-copy">
                <span className="step-label">{t("stepOf", { current: "3", total: "3" })}</span>
                <h2>{t("installComplete")}</h2>
                <p>Aseprite {completedInstallation.version?.replace(/^v/, "") ?? selectedTag.replace(/^v/, "")}</p>
              </div>
              <div className="primary-actions">
                <button className="button primary full" onClick={() => void api.launchInstallation(completedInstallation.id)}>▶ {t("openAseprite")}</button>
                <button className="button ghost full" onClick={returnToStatus}>{t("done")}</button>
              </div>
            </>
          ) : (
            <>
              <div className="flow-heading progress-heading">
                <span className="step-label">{t("stepOf", { current: "3", total: "3" })}</span>
                <h2>{progress.stage === "failed" ? t("installFailed") : t("installingTitle")}</h2>
                <p>{progress.message || t("preparingBuild")}</p>
              </div>
              <div className="progress-copy"><strong>{progress.percent === null ? "…" : `${progress.percent}%`}</strong></div>
              <div className="progress-track" role="progressbar" aria-valuenow={progress.percent ?? undefined}>
                <div className={progress.percent === null ? "indeterminate" : ""} style={{ width: progress.percent === null ? "35%" : `${progress.percent}%` }} />
              </div>
              <p className="build-note">{t("buildingCanTake")}</p>
              <div className="progress-buttons">
                {busy ? (
                  <button className="button danger ghost" onClick={() => void api.cancelOperation()}>{t("cancel")}</button>
                ) : (
                  <button className="button ghost" onClick={() => setView("preflight")}>← {t("back")}</button>
                )}
              </div>
              <details className="logs" open={progress.stage === "failed"}>
                <summary>{t("logs")}</summary>
                <pre>{logs.join("\n") || "…"}</pre>
              </details>
            </>
          )}
        </section>
      )}

      {showEula && (
        <div className="modal-backdrop" role="presentation">
          <section className="modal" role="dialog" aria-modal="true" aria-labelledby="legal-title">
            <button className="modal-close" aria-label={t("close")} onClick={() => setShowEula(false)}>×</button>
            <span className="modal-icon" aria-hidden="true">§</span>
            <h2 id="legal-title">{flowTarget?.channel === "manual" ? t("adoptionTitle") : t("legalTitle")}</h2>
            {flowTarget?.channel === "manual" && <p>{t("adoptionBody")}</p>}
            <p>{t("legalBody")}</p>
            <button className="text-link" onClick={() => void api.openExternal(EULA_URL)}>{t("readEula")} ↗</button>
            <label className="consent">
              <input type="checkbox" checked={eulaAccepted} onChange={(event) => setEulaAccepted(event.target.checked)} />
              <span>{t("legalConfirm")}</span>
            </label>
            <button className="button primary full" disabled={!eulaAccepted} onClick={() => void startInstall()}>{t("continue")} →</button>
          </section>
        </div>
      )}
    </main>
  );
}

export default App;
