const messages = {
  checking: "Checking {platform}…",
  checkingInstallation: "Looking for Aseprite…",
  installationSteps: "Installation steps",
  releaseStep: "Release",
  toolsStep: "Requirements",
  installStep: "Install",
  stepOf: "Step {current} of {total}",
  alreadyInstalled: "Aseprite is already installed",
  notInstalled: "Aseprite is not installed",
  notInstalledHint:
    "Choose the official package or build a verified source release for your personal use.",
  officialEdition: "Official version",
  recommended: "Recommended",
  officialEditionBody:
    "Support Igara Studio and get a signed app, automatic updates, a Steam key, and priority email support.",
  installedSupportTitle: "Support Aseprite",
  officialCopy: "Official copy",
  installedSupportBody:
    "If this is a personal source build, buying an official copy supports Igara Studio and adds signed packages, automatic updates, a Steam key, and priority support.",
  buyOfficial: "Buy official Aseprite — $19.99+",
  orCompile: "or compile it locally",
  compilePersonalCopy: "Compile a personal copy",
  unofficialNotice:
    "This installer is not a free edition of Aseprite and is not affiliated with Igara Studio.",
  openAseprite: "Open Aseprite",
  installAseprite: "Install Aseprite",
  changeVersion: "Reinstall or change version",
  manageInstallation: "Manage this installation",
  installSeparateCopy: "Install a separate copy",
  manualStatusHint:
    "This copy was installed manually. It can be adopted safely before replacement.",
  manualReadOnlyHint:
    "This manual copy cannot be replaced by the current account. Install a separate managed copy at {path}.",
  managedReadOnlyHint:
    "This managed copy is no longer writable by the current account. Install a separate copy at {path} or restore its permissions outside the installer.",
  moreOptions: "More options",
  otherInstallations: "Other detected installations ({count})",
  back: "Back",
  chooseVersionTitle: "Choose the source release",
  chooseVersionBody:
    "Only verified Aseprite 1.3.x source archives are offered.",
  defaultInstallTarget: "Default location on {platform}: {path}",
  selectedInstallTarget: "Selected installation location: {path}",
  loadingReleases: "Loading official releases…",
  continueToChecks: "Check requirements",
  checkToolsTitle: "Check build requirements",
  checkToolsBody:
    "Blocking checks must pass. Differences from Aseprite’s documented tested baseline are warnings when the functional build test succeeds.",
  checkingTools: "Checking requirements…",
  fixRequirements: "Resolve the missing requirements to continue.",
  checkAgain: "Check again",
  resolveRequirement: "How to resolve",
  reviewWarning: "Why this warning?",
  installingTitle: "Compiling and installing Aseprite",
  preparingBuild: "Preparing the build…",
  installComplete: "Aseprite is ready",
  installFailed: "Installation stopped",
  done: "Done",
  restoreComplete: "The previous installation was restored.",
  uninstallComplete: "The managed application was moved to the {trash}.",
  unsupportedPlatform: "Unsupported {platform} configuration",
  unsupportedPlatformFallback:
    "This architecture does not have an officially supported Aseprite build path.",
  managed: "Managed",
  manual: "Manual",
  steam: "Steam",
  packageManager: "Package manager",
  unknownVersion: "Unknown version",
  open: "Open",
  reveal: "Show in {fileManager}",
  restore: "Restore previous",
  uninstall: "Uninstall",
  includePrereleases: "Include beta and RC releases",
  latest: "Latest",
  beta: "Pre-release",
  selectRelease: "Select a release",
  releaseTag: "GitHub tag {tag}",
  installTools: "Install CMake and Ninja with Homebrew",
  install: "Compile and install",
  update: "Compile update",
  downgrade: "Compile older release",
  reinstall: "Recompile release",
  cancel: "Cancel",
  logs: "Logs",
  cleanCache: "Clean cache",
  cacheCleaned: "Cache cleaned ({size})",
  legalTitle: "Personal-use compilation",
  legalBody:
    "Aseprite’s license allows you to compile and modify its source code for your own personal purpose. This installer downloads the selected official source archive and builds it locally; it does not redistribute Aseprite.",
  supportTitle: "Support the people who make Aseprite",
  supportBody:
    "Buying the official version funds continued development and includes signed packages, automatic updates, and priority support.",
  buyInstead: "Buy the official version instead",
  legalConfirm:
    "I have read the Aseprite EULA and confirm this build is for my personal use.",
  readEula: "Read the Aseprite EULA",
  continue: "Accept and start",
  close: "Close",
  operationFailed: "The operation could not be completed.",
  recoveryBlockedTitle: "An interrupted installation needs attention",
  recoveryBlockedBody:
    "Aseprite Installer is open in safe read-only mode. It will not launch or change Aseprite until the recorded transaction is recovered.",
  recoveryJournal: "Recovery journal: {path}",
  retryRecovery: "Retry safe recovery",
  retryingRecovery: "Recovering…",
  recoveryComplete: "The interrupted installation was recovered safely.",
  retry: "Try again",
  restoreTitle: "Restore the previous installation?",
  confirmRestore:
    "The current managed application will be replaced with its previous backup.",
  confirmRestoreAction: "Restore previous",
  uninstallTitle: "Uninstall Aseprite?",
  confirmUninstall:
    "Only the managed application will be moved to the {trash}. Your preferences and artwork will not be removed.",
  confirmUninstallAction: "Uninstall",
  restoring: "Restoring…",
  uninstalling: "Uninstalling…",
  installingTools: "Installing build tools…",
  cancelToolInstall: "Cancel Homebrew installation",
  externalReadOnly:
    "This installation is managed by its original channel and is read-only here.",
  adoptionTitle: "Adopt manual installation",
  adoptionBody:
    "The existing app will be backed up before the selected release replaces it. If its folder is not writable, a managed copy will be installed at {path} instead.",
  buildingCanTake:
    "Compilation can take several minutes and temporarily use 3–6 GB.",
  finishingSafely:
    "Finishing a protected installation step. Cancellation will be available only after the transaction is safe.",
  supportAfterInstall:
    "If Aseprite becomes part of your workflow, consider buying an official copy to support its continued development.",
  supportDevelopment: "Support Aseprite development",
} as const;

export type MessageKey = keyof typeof messages;

export function getLocale(): "en" {
  return "en";
}

export function createTranslator(_locale: "en") {
  return (key: MessageKey, values?: Record<string, string>) => {
    let value: string = messages[key];
    for (const [name, replacement] of Object.entries(values ?? {})) {
      value = value.replace(`{${name}}`, replacement);
    }
    return value;
  };
}
