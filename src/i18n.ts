const messages = {
  checking: "Checking your Mac…",
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
  moreOptions: "More options",
  otherInstallations: "Other detected installations ({count})",
  back: "Back",
  chooseVersionTitle: "Choose the source release",
  chooseVersionBody:
    "Only verified Aseprite 1.3.x source archives are offered.",
  loadingReleases: "Loading official releases…",
  continueToChecks: "Check requirements",
  checkToolsTitle: "Check build requirements",
  checkToolsBody:
    "Everything below is required before Aseprite can be compiled locally.",
  checkingTools: "Checking requirements…",
  fixRequirements: "Resolve the missing requirements to continue.",
  checkAgain: "Check again",
  installManually: "Install manually",
  installingTitle: "Compiling and installing Aseprite",
  preparingBuild: "Preparing the build…",
  installComplete: "Aseprite is ready",
  installFailed: "Installation stopped",
  done: "Done",
  restoreComplete: "The previous installation was restored.",
  uninstallComplete: "The managed application was moved to the Trash.",
  managed: "Managed",
  manual: "Manual",
  steam: "Steam",
  packageManager: "Package manager",
  unknownVersion: "Unknown version",
  open: "Open",
  reveal: "Show in Finder",
  restore: "Restore previous",
  uninstall: "Uninstall",
  includePrereleases: "Include beta and RC releases",
  latest: "Latest",
  beta: "Pre-release",
  selectRelease: "Select a release",
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
  retry: "Try again",
  restoreTitle: "Restore the previous installation?",
  confirmRestore:
    "The current managed application will be replaced with its previous backup.",
  confirmRestoreAction: "Restore previous",
  uninstallTitle: "Uninstall Aseprite?",
  confirmUninstall:
    "Only the managed application will be moved. Your preferences and artwork will not be removed.",
  confirmUninstallAction: "Uninstall",
  restoring: "Restoring…",
  uninstalling: "Uninstalling…",
  installingTools: "Installing build tools…",
  externalReadOnly:
    "This installation is managed by its original channel and is read-only here.",
  adoptionTitle: "Adopt manual installation",
  adoptionBody:
    "The existing app will be backed up before the selected release replaces it. If its folder is not writable, a managed copy will be installed in ~/Applications instead.",
  buildingCanTake:
    "Compilation can take several minutes and temporarily use 3–6 GB.",
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
