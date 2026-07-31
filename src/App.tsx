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

const EULA_URL =
  "https://github.com/aseprite/aseprite/blob/main/EULA.txt";
const ASEPRITE_URL = "https://www.aseprite.org/";
const PROJECT_URL = "https://github.com/fmhun/asprite-installer";

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

function App() {
  const t = useMemo(() => createTranslator(getLocale()), []);
  const [releases, setReleases] = useState<ReleaseInfo[]>([]);
  const [installations, setInstallations] = useState<InstallationInfo[]>([]);
  const [preflight, setPreflight] = useState<PreflightReport | null>(null);
  const [includePrereleases, setIncludePrereleases] = useState(false);
  const [selectedTag, setSelectedTag] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [installingTools, setInstallingTools] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [progress, setProgress] =
    useState<OperationProgress>(initialProgress);
  const [logs, setLogs] = useState<string[]>([]);
  const [showEula, setShowEula] = useState(false);
  const [eulaAccepted, setEulaAccepted] = useState(false);
  const [adoption, setAdoption] = useState<InstallationInfo | null>(null);

  const refresh = useCallback(
    async (showSpinner = true) => {
      if (showSpinner) setLoading(true);
      setError(null);
      try {
        const [releaseData, installationData, preflightData] =
          await Promise.all([
            api.listReleases(includePrereleases),
            api.scanInstallations(),
            api.runPreflight(),
          ]);
        setReleases(releaseData);
        setInstallations(installationData);
        setPreflight(preflightData);
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
        setLoading(false);
      }
    },
    [includePrereleases],
  );

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const selectedRelease = releases.find(
    (release) => release.tag === selectedTag,
  );
  const managed = installations.find(
    (installation) => installation.channel === "managed",
  );

  const actionLabel = useMemo(() => {
    if (!managed) return t("install");
    const comparison = compareVersions(selectedTag, managed.version);
    if (comparison > 0) return t("update");
    if (comparison < 0) return t("downgrade");
    return t("reinstall");
  }, [managed, selectedTag, t]);

  const beginConsent = (installation: InstallationInfo | null = null) => {
    setAdoption(installation);
    setEulaAccepted(false);
    setShowEula(true);
  };

  const startInstall = async () => {
    if (!selectedRelease || !eulaAccepted) return;
    const request: InstallRequest = {
      tag: selectedRelease.tag,
      targetPath: adoption?.path ?? managed?.path ?? null,
      adopt: Boolean(adoption),
      eulaAccepted,
    };
    setShowEula(false);
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
      await api.startInstall(request, (event) => {
        setProgress(event);
        if (event.logLine) {
          setLogs((current) => [...current.slice(-499), event.logLine!]);
        }
      });
      await refresh(false);
    } catch (caught) {
      setError(errorMessage(caught));
      setProgress((current) => ({ ...current, stage: "failed" }));
    } finally {
      setBusy(false);
      setAdoption(null);
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
      await refresh(false);
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
      await refresh(false);
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

  if (loading && !releases.length && !installations.length) {
    return (
      <main className="loading-screen">
        <img src="/icon.png" alt="" />
        <div className="spinner" aria-hidden="true" />
        <p>{t("checking")}</p>
      </main>
    );
  }

  return (
    <main className="app-shell">
      <header className="app-header">
        <div className="brand">
          <img className="app-icon" src="/icon.png" alt="" />
          <div>
            <div className="eyebrow">{t("unofficial")}</div>
            <h1>Aseprite Installer</h1>
            <p>{t("tagline")}</p>
          </div>
        </div>
        <button
          className="button secondary compact"
          disabled={busy}
          onClick={() => void refresh()}
        >
          ↻ {t("refresh")}
        </button>
      </header>

      {error && (
        <div className="alert error" role="alert">
          <strong>{t("operationFailed")}</strong>
          <span>{error}</span>
          <button onClick={() => setError(null)}>×</button>
        </div>
      )}
      {notice && (
        <div className="alert success" role="status">
          <span>{notice}</span>
          <button onClick={() => setNotice(null)}>×</button>
        </div>
      )}

      <section className="panel">
        <div className="section-heading">
          <div>
            <span className="step-number">01</span>
            <h2>{t("currentInstallations")}</h2>
          </div>
          <span className="count">{installations.length}</span>
        </div>

        {installations.length === 0 ? (
          <div className="empty-state">
            <div className="empty-pixel" aria-hidden="true">
              +
            </div>
            <div>
              <strong>{t("noInstallation")}</strong>
              <p>{t("noInstallationHint")}</p>
            </div>
          </div>
        ) : (
          <div className="installation-list">
            {installations.map((installation) => (
              <article className="installation-card" key={installation.id}>
                <div className="installation-main">
                  <span className={`channel ${installation.channel}`}>
                    {t(installation.channel)}
                  </span>
                  <h3>
                    Aseprite{" "}
                    <span>
                      {installation.version
                        ? installation.version.replace(/^v/, "")
                        : t("unknownVersion")}
                    </span>
                  </h3>
                  <p className="path" title={installation.path}>
                    {installation.path}
                  </p>
                  <small>
                    {installation.versionExact
                      ? t("exactVersion")
                      : t("partialVersion")}
                    {installation.architecture
                      ? ` · ${installation.architecture}`
                      : ""}
                  </small>
                </div>
                <div className="installation-actions">
                  <button
                    className="button ghost"
                    onClick={() => void api.launchInstallation(installation.id)}
                  >
                    ▶ {t("open")}
                  </button>
                  <button
                    className="button ghost"
                    onClick={() => void api.revealInstallation(installation.id)}
                  >
                    ⌖ {t("reveal")}
                  </button>
                  {installation.channel === "manual" && (
                      <button
                        className="button secondary"
                        disabled={busy}
                        onClick={() => beginConsent(installation)}
                      >
                        {t("adopt")}
                      </button>
                    )}
                  {installation.channel === "managed" && (
                    <>
                      {installation.hasBackup && (
                        <button
                          className="button ghost"
                          disabled={busy}
                          onClick={() => void restore(installation)}
                        >
                          ↶ {t("restore")}
                        </button>
                      )}
                      <button
                        className="button danger ghost"
                        disabled={busy}
                        onClick={() => void uninstall(installation)}
                      >
                        {t("uninstall")}
                      </button>
                    </>
                  )}
                </div>
                {installation.channel === "manual" && (
                  <p className="card-note">{t("manualHint")}</p>
                )}
                {(installation.channel === "steam" ||
                  installation.channel === "packageManager") && (
                  <p className="card-note">{t("externalReadOnly")}</p>
                )}
              </article>
            ))}
          </div>
        )}
      </section>

      <div className="two-column">
        <section className="panel">
          <div className="section-heading">
            <div>
              <span className="step-number">02</span>
              <h2>{t("sourceRelease")}</h2>
            </div>
          </div>

          <label className="field-label" htmlFor="release">
            {t("selectRelease")}
          </label>
          <select
            id="release"
            value={selectedTag}
            disabled={busy}
            onChange={(event) => setSelectedTag(event.target.value)}
          >
            {releases.map((release) => (
              <option key={release.tag} value={release.tag}>
                {release.tag.replace(/^v/, "")}
                {release.latest ? ` — ${t("latest")}` : ""}
                {release.prerelease ? ` — ${t("beta")}` : ""}
              </option>
            ))}
          </select>
          {selectedRelease && (
            <div className="release-meta">
              <span>
                {new Date(selectedRelease.publishedAt).toLocaleDateString()}
              </span>
              <span>{formatBytes(selectedRelease.size)}</span>
              <span className="checksum">SHA-256 ✓</span>
            </div>
          )}
          <label className="switch-row">
            <input
              type="checkbox"
              checked={includePrereleases}
              disabled={busy}
              onChange={(event) =>
                setIncludePrereleases(event.target.checked)
              }
            />
            <span>
              <strong>{t("includePrereleases")}</strong>
            </span>
          </label>
        </section>

        <section className="panel">
          <div className="section-heading">
            <div>
              <span className="step-number">03</span>
              <h2>{t("environment")}</h2>
            </div>
            {preflight && (
              <span className={`status-pill ${preflight.ready ? "ok" : "warn"}`}>
                {preflight.ready ? t("ready") : t("notReady")}
              </span>
            )}
          </div>
          {preflight && (
            <>
              <ul className="check-list">
                {preflight.prerequisites.map((item) => (
                  <li className={item.ok ? "ok" : "missing"} key={item.id}>
                    <span className="check-mark">{item.ok ? "✓" : "!"}</span>
                    <div>
                      <strong>{item.label}</strong>
                      <small>{item.detail}</small>
                    </div>
                  </li>
                ))}
              </ul>
              {preflight.homebrewAvailable &&
                preflight.prerequisites.some(
                  (item) =>
                    !item.ok && (item.id === "cmake" || item.id === "ninja"),
                ) && (
                  <button
                    className="button secondary full"
                    disabled={installingTools || busy}
                    onClick={() => void installTools()}
                  >
                    {installingTools ? t("installingTools") : t("installTools")}
                  </button>
                )}
            </>
          )}
        </section>
      </div>

      <section className={`action-panel ${busy ? "is-busy" : ""}`}>
        {busy ? (
          <>
            <div className="progress-copy">
              <div>
                <span className="pulse" aria-hidden="true" />
                <strong>{progress.message || t("progress")}</strong>
              </div>
              <span>
                {progress.percent === null ? "…" : `${progress.percent}%`}
              </span>
            </div>
            <div
              className="progress-track"
              role="progressbar"
              aria-valuenow={progress.percent ?? undefined}
            >
              <div
                className={progress.percent === null ? "indeterminate" : ""}
                style={{
                  width:
                    progress.percent === null ? "35%" : `${progress.percent}%`,
                }}
              />
            </div>
            <div className="progress-actions">
              <small>{t("buildingCanTake")}</small>
              <button
                className="button danger ghost"
                onClick={() => void api.cancelOperation()}
              >
                {t("cancel")}
              </button>
            </div>
            <details className="logs" open={progress.stage === "failed"}>
              <summary>{t("logs")}</summary>
              <pre>{logs.join("\n") || "…"}</pre>
            </details>
          </>
        ) : (
          <>
            <div>
              <strong>
                {selectedRelease?.name ?? selectedRelease?.tag ?? "Aseprite"}
              </strong>
              <p>{t("buildingCanTake")}</p>
            </div>
            <button
              className="button primary large"
              disabled={!selectedRelease || !preflight?.ready}
              onClick={() => beginConsent()}
            >
              <span>{actionLabel}</span>
              <span aria-hidden="true">→</span>
            </button>
          </>
        )}
      </section>

      <footer>
        <div>
          <strong>{t("aboutLinks")}</strong>
          <button onClick={() => void api.openExternal(ASEPRITE_URL)}>
            {t("asepriteWebsite")} ↗
          </button>
          <button onClick={() => void api.openExternal(PROJECT_URL)}>
            {t("sourceCode")} ↗
          </button>
        </div>
        <div className="footer-tools">
          <span>{t("privacy")}</span>
          <button className="button ghost compact" onClick={() => void cleanCache()}>
            {t("cleanCache")}
          </button>
        </div>
      </footer>

      {showEula && (
        <div className="modal-backdrop" role="presentation">
          <section
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="legal-title"
          >
            <button
              className="modal-close"
              aria-label={t("close")}
              onClick={() => setShowEula(false)}
            >
              ×
            </button>
            <span className="modal-icon" aria-hidden="true">
              §
            </span>
            <h2 id="legal-title">
              {adoption ? t("adoptionTitle") : t("legalTitle")}
            </h2>
            {adoption && <p>{t("adoptionBody")}</p>}
            <p>{t("legalBody")}</p>
            <button
              className="text-link"
              onClick={() => void api.openExternal(EULA_URL)}
            >
              {t("readEula")} ↗
            </button>
            <label className="consent">
              <input
                type="checkbox"
                checked={eulaAccepted}
                onChange={(event) => setEulaAccepted(event.target.checked)}
              />
              <span>{t("legalConfirm")}</span>
            </label>
            <button
              className="button primary full"
              disabled={!eulaAccepted}
              onClick={() => void startInstall()}
            >
              {t("continue")} →
            </button>
          </section>
        </div>
      )}
    </main>
  );
}

export default App;
