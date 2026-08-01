export type InstallationChannel =
  | "managed"
  | "manual"
  | "steam"
  | "packageManager";

export type OperationStage =
  | "idle"
  | "preflight"
  | "downloading"
  | "verifying"
  | "extracting"
  | "compiling"
  | "preparingArtifact"
  | "signing"
  | "backingUp"
  | "installing"
  | "integrating"
  | "finalizing"
  | "validating"
  | "rollingBack"
  | "completed"
  | "failed"
  | "cancelled";

export interface ReleaseInfo {
  tag: string;
  name: string;
  publishedAt: string;
  prerelease: boolean;
  latest: boolean;
  sourceAssetName: string;
  sourceUrl: string;
  digest: string;
  size: number;
}

export interface InstallationInfo {
  id: string;
  path: string;
  version: string | null;
  versionExact: boolean;
  architecture: string | null;
  channel: InstallationChannel;
  manageable: boolean;
  writable: boolean;
  hasBackup: boolean;
  installedAt: string | null;
}

export interface Prerequisite {
  id: string;
  label: string;
  ok: boolean;
  required: boolean;
  detail: string;
  remediation: string | null;
}

export interface PreflightReport {
  ready: boolean;
  architecture: string;
  osVersion: string;
  freeBytes: number;
  minimumFreeBytes: number;
  homebrewAvailable: boolean;
  prerequisites: Prerequisite[];
}

export interface PlatformInfo {
  id: "macos" | "windows" | "linux";
  displayName: string;
  architecture: string;
  supported: boolean;
  unsupportedReason: string | null;
  defaultTargetPath: string;
  fileManagerName: string;
  trashName: string;
  shellName: string;
}

export interface RecoveryStatus {
  blocked: boolean;
  message: string | null;
  detail: string | null;
  journalPath: string | null;
}

export interface OperationProgress {
  stage: OperationStage;
  percent: number | null;
  message: string;
  logLine: string | null;
  canCancel: boolean;
}

export interface InstallerError {
  code: string;
  message: string;
  detail?: string | null;
}

export interface InstallRequest {
  tag: string;
  targetPath: string | null;
  adopt: boolean;
  eulaAccepted: boolean;
}
