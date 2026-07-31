export interface HelpStep {
  title: string;
  body: string;
  command?: string;
}

export interface HelpLink {
  label: string;
  url: string;
}

export interface PrerequisiteGuide {
  title: string;
  summary: string;
  steps: HelpStep[];
  links: HelpLink[];
}

const ASEPRITE_INSTALL_URL =
  "https://github.com/aseprite/aseprite/blob/main/INSTALL.md";

const guides: Record<string, PrerequisiteGuide> = {
  macos: {
    title: "Update macOS",
    summary:
      "Aseprite’s supported macOS baseline is macOS 15.2. Install the newest macOS version that Software Update offers for this Mac.",
    steps: [
      {
        title: "Back up important files",
        body: "Apple recommends making a backup before installing a macOS upgrade.",
      },
      {
        title: "Open Software Update",
        body: "Choose Apple menu › System Settings › General › Software Update.",
      },
      {
        title: "Install a compatible update",
        body: "Install macOS 15.2 or newer, restart if requested, then return here and run the requirements check again.",
      },
    ],
    links: [
      { label: "Apple: Update macOS", url: "https://support.apple.com/en-us/108382" },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  xcode: {
    title: "Install and select Xcode",
    summary:
      "Aseprite’s documented macOS baseline uses Xcode 16.3 and the macOS 15.4 SDK. A newer compatible Xcode version can also provide the required compiler and SDK.",
    steps: [
      {
        title: "Install Xcode",
        body: "Install Xcode from Apple, open it once, allow its additional components to finish installing, and complete Apple’s license prompts.",
      },
      {
        title: "Select the installed Xcode",
        body: "If Xcode is installed in /Applications/Xcode.app, select its developer directory in Terminal.",
        command: "sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer",
      },
      {
        title: "Verify the compiler",
        body: "Both commands must complete without an error.",
        command: "xcode-select -p\nxcrun clang --version",
      },
    ],
    links: [
      {
        label: "Apple: Command-line tools",
        url: "https://developer.apple.com/documentation/xcode/installing-the-command-line-tools",
      },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  sdk: {
    title: "Configure the macOS SDK",
    summary:
      "The macOS SDK is bundled with Xcode. Aseprite’s documented baseline is the macOS 15.4 SDK; the selected Xcode must expose a compatible SDK through xcrun.",
    steps: [
      {
        title: "Verify the selected SDK",
        body: "This command should print a path ending in MacOSX.sdk.",
        command: "xcrun --sdk macosx --show-sdk-path",
      },
      {
        title: "Select Xcode if the command fails",
        body: "Point xcode-select to the full Xcode installation, then run the SDK command again.",
        command: "sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer",
      },
      {
        title: "Finish Xcode setup",
        body: "Open Xcode and let it install requested platform components before checking requirements again.",
      },
    ],
    links: [
      {
        label: "Apple: Command-line tools",
        url: "https://developer.apple.com/documentation/xcode/installing-the-command-line-tools",
      },
      { label: "Aseprite macOS build instructions", url: ASEPRITE_INSTALL_URL },
    ],
  },
  cmake: {
    title: "Install CMake",
    summary:
      "Aseprite requires the latest CMake. Homebrew is the simplest supported route when it is already installed; the official universal macOS package is the manual alternative.",
    steps: [
      {
        title: "With Homebrew",
        body: "Install the current stable CMake formula.",
        command: "brew install cmake",
      },
      {
        title: "Without Homebrew",
        body: "Download the universal macOS .dmg from cmake.org, install CMake.app, then follow CMake’s in-app command-line setup instructions so the cmake command is on PATH.",
      },
      {
        title: "Verify",
        body: "Restart the installer after changing PATH if necessary, then verify CMake in Terminal.",
        command: "cmake --version",
      },
    ],
    links: [
      { label: "Official CMake downloads", url: "https://cmake.org/download/" },
      { label: "Homebrew CMake formula", url: "https://formulae.brew.sh/formula/cmake" },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  ninja: {
    title: "Install Ninja",
    summary:
      "Aseprite uses Ninja as its build system. Install the Homebrew formula or the official macOS release binary, then make sure ninja is on PATH.",
    steps: [
      {
        title: "With Homebrew",
        body: "Install the current stable Ninja formula.",
        command: "brew install ninja",
      },
      {
        title: "Without Homebrew",
        body: "Download the macOS archive from Ninja’s official GitHub Releases page, extract it, open Terminal in that folder, and install the executable in /usr/local/bin.",
        command: "sudo install -m 0755 ./ninja /usr/local/bin/ninja",
      },
      {
        title: "Verify",
        body: "Restart the installer after changing PATH if necessary, then verify Ninja in Terminal.",
        command: "ninja --version",
      },
    ],
    links: [
      { label: "Official Ninja releases", url: "https://github.com/ninja-build/ninja/releases" },
      { label: "Homebrew Ninja formula", url: "https://formulae.brew.sh/formula/ninja" },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  disk: {
    title: "Free disk space",
    summary:
      "The local Aseprite build needs at least 6 GB free in the installer cache volume. This space is temporary and most build artifacts are removed after success.",
    steps: [
      {
        title: "Review storage",
        body: "Choose Apple menu › System Settings › General › Storage to see available space and Apple’s recommendations.",
      },
      {
        title: "Free at least 6 GB",
        body: "Move or remove unneeded downloads, applications, media, or other large files. Empty the Trash if you want trashed files to release their disk space.",
      },
      {
        title: "Check again",
        body: "Return to Aseprite Installer and run the requirements check again.",
      },
    ],
    links: [
      { label: "Apple: Free up storage space", url: "https://support.apple.com/en-us/102624" },
    ],
  },
};

export function getPrerequisiteGuide(id: string): PrerequisiteGuide {
  return guides[id] ?? {
    title: "Resolve this requirement",
    summary: "Follow the requirement details below, then run the check again.",
    steps: [],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  };
}
