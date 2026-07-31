import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  InstallationInfo,
  InstallRequest,
  OperationProgress,
  PreflightReport,
  ReleaseInfo,
} from "./types";

export const api = {
  listReleases: (includePrereleases: boolean) =>
    invoke<ReleaseInfo[]>("list_releases", { includePrereleases }),
  scanInstallations: () =>
    invoke<InstallationInfo[]>("scan_installations"),
  runPreflight: () => invoke<PreflightReport>("run_preflight"),
  installBuildTools: () => invoke<PreflightReport>("install_build_tools"),
  startInstall: (
    request: InstallRequest,
    onProgress: (progress: OperationProgress) => void,
  ) => {
    const channel = new Channel<OperationProgress>();
    channel.onmessage = onProgress;
    return invoke<InstallationInfo>("start_install", {
      request,
      progress: channel,
    });
  },
  cancelOperation: () => invoke<void>("cancel_operation"),
  launchInstallation: (id: string) =>
    invoke<void>("launch_installation", { id }),
  revealInstallation: (id: string) =>
    invoke<void>("reveal_installation", { id }),
  restorePrevious: (id: string) =>
    invoke<InstallationInfo>("restore_previous", { id }),
  uninstallManaged: (id: string) =>
    invoke<void>("uninstall_managed", { id }),
  cleanCache: () => invoke<number>("clean_cache"),
  openExternal: (url: string) => invoke<void>("open_external", { url }),
};
