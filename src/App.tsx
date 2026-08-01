import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "./api";
import { createTranslator, getLocale } from "./i18n";
import type {
  InstallationInfo,
  InstallRequest,
  InstallerError,
  OperationProgress,
  PreflightReport,
  Prerequisite,
  ReleaseInfo,
} from "./types";
import { AppFooter } from "./components/AppFooter";
import { LoadingIndicator } from "./components/LoadingIndicator";
import { Modal, PixelDocumentIcon } from "./components/Modal";
import { PrerequisiteHelpModal } from "./components/PrerequisiteHelpModal";
import { withMinimumDuration } from "./timing";
import { compareVersions } from "./version";
import { useFixedWindowHeight } from "./windowSizing";

const EULA_URL = "https://github.com/aseprite/aseprite/blob/main/EULA.txt";
const BUY_URL = "https://www.aseprite.org/buy/";

type View = "status" | "release" | "preflight" | "install";
type PendingAction = {
  kind: "restore" | "uninstall";
  installation: InstallationInfo;
};

const flowSteps = ["release", "preflight", "install"] as const;

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
  useFixedWindowHeight();
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
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [helpPrerequisite, setHelpPrerequisite] = useState<Prerequisite | null>(null);

  const refreshInstallations = useCallback(async (showSpinner = true) => {
    if (showSpinner) setLoading(true);
    setError(null);
    try {
      const scan = api.scanInstallations();
      setInstallations(
        await (showSpinner ? withMinimumDuration(scan) : scan),
      );
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
      const releaseData = await withMinimumDuration(
        api.listReleases(includePrereleases),
      );
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
  const hasMultipleInstallations = sortedInstallations.length > 1;
  const selectedRelease = releases.find((release) => release.tag === selectedTag);
  const currentStepIndex = flowSteps.indexOf(
    view === "status" ? "release" : view,
  );
  const flowComplete = view === "install" && completedInstallation !== null;

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
    setReleaseLoading(true);
    setView("release");
  };

  const openPreflight = async () => {
    setView("preflight");
    setPreflightLoading(true);
    setError(null);
    try {
      setPreflight(await withMinimumDuration(api.runPreflight()));
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setPreflightLoading(false);
    }
  };

  const confirmPreflight = async () => {
    setPreflightLoading(true);
    setError(null);
    try {
      const refreshedPreflight = await withMinimumDuration(api.runPreflight());
      setPreflight(refreshedPreflight);
      if (!refreshedPreflight.ready) return;
      setEulaAccepted(false);
      setShowEula(true);
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
      const message = errorMessage(caught);
      setProgress((current) => ({
        ...current,
        stage: "failed",
        percent: current.percent === null ? 0 : Math.min(100, Math.max(0, current.percent)),
        message,
        logLine: null,
      }));
      setLogs((current) => [...current.slice(-499), `ERROR: ${message}`]);
    } finally {
      setBusy(false);
    }
  };

  const installTools = async () => {
    setInstallingTools(true);
    setError(null);
    try {
      setPreflight(await withMinimumDuration(api.installBuildTools()));
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setInstallingTools(false);
    }
  };

  const confirmManagedAction = async () => {
    if (!pendingAction) return;
    const { kind, installation } = pendingAction;
    setBusy(true);
    setError(null);
    setNotice(null);
    setActionError(null);
    try {
      if (kind === "restore") {
        await api.restorePrevious(installation.id);
        setNotice(t("restoreComplete"));
      } else {
        await api.uninstallManaged(installation.id);
        setNotice(t("uninstallComplete"));
      }
      await refreshInstallations(false);
      setPendingAction(null);
    } catch (caught) {
      setActionError(errorMessage(caught));
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
      <main className="app-shell loading-layout">
        <LoadingIndicator label={t("checkingInstallation")} screen />
        <AppFooter disclaimer={t("unofficialNotice")} />
      </main>
    );
  }

  return (
    <main className="app-shell">
      <header className="app-header compact-header">
        <div className="brand">
          <img className="app-icon" src="/icon.png" alt="" />
          <h1>Aseprite Installer</h1>
        </div>
      </header>

      {view !== "status" && (
        <nav
          className="stepper"
          data-current-step={currentStepIndex}
          data-flow-complete={flowComplete}
          aria-label={t("installationSteps")}
        >
          {flowSteps.map((step, index) => {
            const isComplete = flowComplete && step === "install";
            const state = isComplete
              ? "complete"
              : index < currentStepIndex
                ? "done"
                : index === currentStepIndex
                  ? "current"
                  : "upcoming";
            return (
              <div
                className={state}
                data-state={state}
                key={step}
              >
                <span aria-current={state === "current" ? "step" : undefined}>
                  {index + 1}
                </span>
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
              {(primaryInstallation.channel === "managed" ||
                primaryInstallation.channel === "manual") && (
                <section className="official-purchase installed-support" aria-labelledby="installed-support-title">
                  <div className="official-purchase-heading">
                    <h3 id="installed-support-title">{t("installedSupportTitle")}</h3>
                    <span>{t("officialCopy")}</span>
                  </div>
                  <p>{t("installedSupportBody")}</p>
                  <button
                    className="button secondary full"
                    onClick={() => void api.openExternal(BUY_URL)}
                  >
                    {t("supportDevelopment")} ↗
                  </button>
                </section>
              )}
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
                    <button className="button ghost compact" disabled={busy} onClick={() => { setActionError(null); setPendingAction({ kind: "restore", installation: primaryInstallation }); }}>
                      {t("restore")}
                    </button>
                  )}
                  {primaryInstallation.channel === "managed" && (
                    <button className="button danger ghost compact" disabled={busy} onClick={() => { setActionError(null); setPendingAction({ kind: "uninstall", installation: primaryInstallation }); }}>
                      {t("uninstall")}
                    </button>
                  )}
                  <button className="button ghost compact" onClick={() => void cleanCache()}>{t("cleanCache")}</button>
                </div>
              </details>
              {hasMultipleInstallations && (
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
              <section className="official-purchase" aria-labelledby="official-edition-title">
                <div className="official-purchase-heading">
                  <h3 id="official-edition-title">{t("officialEdition")}</h3>
                  <span>{t("recommended")}</span>
                </div>
                <p>{t("officialEditionBody")}</p>
                <button
                  className="button primary full"
                  onClick={() => void api.openExternal(BUY_URL)}
                >
                  {t("buyOfficial")} ↗
                </button>
              </section>
              <div className="choice-divider"><span>{t("orCompile")}</span></div>
              <div className="primary-actions">
                <button className="button secondary full" onClick={() => startFlow(null)}>
                  {t("compilePersonalCopy")} →
                </button>
              </div>
              <details className="more-options">
                <summary>{t("moreOptions")}</summary>
                <div>
                  <button className="button ghost compact" onClick={() => void cleanCache()}>{t("cleanCache")}</button>
                </div>
              </details>
            </>
          )}
        </section>
      )}

      {view === "release" && (
        <section className="context-card flow-view">
          <div className="flow-heading">
            <button className="back-button" onClick={returnToStatus}>{"<"} {t("back")}</button>
            <span className="step-label">{t("stepOf", { current: "1", total: "3" })}</span>
            <h2>{t("chooseVersionTitle")}</h2>
            <p>{t("chooseVersionBody")}</p>
          </div>
          {releaseLoading ? (
            <LoadingIndicator label={t("loadingReleases")} />
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
                  <span>{new Date(selectedRelease.publishedAt).toLocaleDateString("en-US")}</span>
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
            <button className="back-button" onClick={() => setView("release")}>{"<"} {t("back")}</button>
            <span className="step-label">{t("stepOf", { current: "2", total: "3" })}</span>
            <h2>{t("checkToolsTitle")}</h2>
            <p>{t("checkToolsBody")}</p>
          </div>
          {preflightLoading ? (
            <LoadingIndicator label={t("checkingTools")} />
          ) : preflight ? (
            <>
              <ul className="check-list single-column">
                {preflight.prerequisites.map((item) => (
                  <li className={item.ok ? "ok" : "missing"} key={item.id}>
                    <span className="check-mark">{item.ok ? "✓" : "!"}</span>
                    <div className="check-copy"><strong>{item.label}</strong><small title={item.detail}>{item.detail}</small></div>
                    {!item.ok && (
                      <button
                        className="manual-help-trigger"
                        type="button"
                        onClick={() => setHelpPrerequisite(item)}
                      >
                        <span aria-hidden="true">?</span>
                        {t("installManually")}
                      </button>
                    )}
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
                disabled={installingTools}
                onClick={() => void confirmPreflight()}
              >
                {preflight.ready ? `${actionLabel} →` : `${t("checkAgain")} ↻`}
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
                <section className="post-install-support" aria-label={t("supportTitle")}>
                  <p>{t("supportAfterInstall")}</p>
                  <button className="button secondary full" onClick={() => void api.openExternal(BUY_URL)}>
                    {t("supportDevelopment")} ↗
                  </button>
                </section>
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
              <div className="progress-copy"><strong>{progress.percent === null && busy ? "…" : `${Math.min(100, Math.max(0, progress.percent ?? 0))}%`}</strong></div>
              <div className="progress-track" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={progress.percent ?? undefined}>
                <div
                  className={progress.percent === null && busy ? "indeterminate" : ""}
                  style={{ width: progress.percent === null && busy ? "35%" : `${Math.min(100, Math.max(0, progress.percent ?? 0))}%` }}
                />
              </div>
              <p className="build-note">{t("buildingCanTake")}</p>
              <div className="progress-buttons">
                {busy ? (
                  <button className="button danger ghost" onClick={() => void api.cancelOperation()}>{t("cancel")}</button>
                ) : progress.stage === "failed" ? (
                  <>
                    <button className="button primary" onClick={() => void startInstall()}>{t("retry")}</button>
                    <button className="button ghost" onClick={() => setView("preflight")}>{"<"} {t("back")}</button>
                  </>
                ) : (
                  <button className="button ghost" onClick={() => setView("preflight")}>{"<"} {t("back")}</button>
                )}
              </div>
              <section className="logs" aria-labelledby="logs-title">
                <h3 id="logs-title">{t("logs")}</h3>
                <pre aria-live="polite">{logs.join("\n") || "…"}</pre>
              </section>
            </>
          )}
        </section>
      )}

      <AppFooter disclaimer={t("unofficialNotice")} />

      {showEula && (
        <Modal
          ariaLabelledBy="legal-title"
          titlebar="PERSONAL BUILD / ASEPRITE"
          onClose={() => setShowEula(false)}
        >
            <span className="modal-icon"><PixelDocumentIcon /></span>
            <h2 id="legal-title">{flowTarget?.channel === "manual" ? t("adoptionTitle") : t("legalTitle")}</h2>
            {flowTarget?.channel === "manual" && <p>{t("adoptionBody")}</p>}
            <p>{t("legalBody")}</p>
            <button className="text-link" onClick={() => void api.openExternal(EULA_URL)}>{t("readEula")} ↗</button>
            <section className="legal-support" aria-labelledby="support-title">
              <h3 id="support-title">{t("supportTitle")}</h3>
              <p>{t("supportBody")}</p>
              <button className="button secondary full" onClick={() => void api.openExternal(BUY_URL)}>
                {t("buyInstead")} ↗
              </button>
            </section>
            <label className="consent">
              <input type="checkbox" checked={eulaAccepted} onChange={(event) => setEulaAccepted(event.target.checked)} />
              <span>{t("legalConfirm")}</span>
            </label>
            <button className="button primary full" disabled={!eulaAccepted} onClick={() => void startInstall()}>{t("continue")} →</button>
        </Modal>
      )}

      {pendingAction && (
        <Modal
          ariaLabelledBy="confirmation-title"
          className="confirmation-modal"
          closeDisabled={busy}
          titlebar="CONFIRM ACTION / ASEPRITE"
          onClose={() => { setActionError(null); setPendingAction(null); }}
        >
            <span className="modal-icon" aria-hidden="true">{pendingAction.kind === "restore" ? "↺" : "×"}</span>
            <h2 id="confirmation-title">{t(pendingAction.kind === "restore" ? "restoreTitle" : "uninstallTitle")}</h2>
            <p>{t(pendingAction.kind === "restore" ? "confirmRestore" : "confirmUninstall")}</p>
            <p className="confirmation-path" title={pendingAction.installation.path}>{pendingAction.installation.path}</p>
            {actionError && <p className="confirmation-error" role="alert">{actionError}</p>}
            <div className="confirmation-actions">
              <button className="button ghost" disabled={busy} onClick={() => { setActionError(null); setPendingAction(null); }}>{t("cancel")}</button>
              <button className={`button ${pendingAction.kind === "uninstall" ? "danger" : "primary"}`} disabled={busy} onClick={() => void confirmManagedAction()}>
                {busy
                  ? t(pendingAction.kind === "restore" ? "restoring" : "uninstalling")
                  : t(pendingAction.kind === "restore" ? "confirmRestoreAction" : "confirmUninstallAction")}
              </button>
            </div>
        </Modal>
      )}

      {helpPrerequisite && (
        <PrerequisiteHelpModal
          prerequisite={helpPrerequisite}
          onClose={() => setHelpPrerequisite(null)}
        />
      )}
    </main>
  );
}

export default App;
