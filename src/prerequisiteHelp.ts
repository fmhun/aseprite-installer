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
  nonElevated: {
    title: "Run the installer as your normal user",
    summary:
      "Aseprite Installer must not run through sudo or as root. An elevated launch can select the wrong home folder and leave root-owned cache, lock, backup, or app files behind.",
    steps: [
      {
        title: "Quit the elevated copy",
        body: "Close this installer window. Do not repair the problem by granting broader system permissions or disabling SIP.",
      },
      {
        title: "Open it normally",
        body: "Launch Aseprite Installer from Finder while signed in to the account that will use Aseprite.",
      },
      {
        title: "Check again",
        body: "No Mac restart is required. If a previous sudo launch left root-owned files, ask an administrator to restore ownership of the exact paths reported by the check.",
      },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  macos: {
    title: "Update macOS",
    summary:
      "This installer app targets macOS 15.2 or newer. Aseprite documents 15.2 as its tested macOS baseline but notes that other versions can work.",
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
  architecture: {
    title: "Use a supported build architecture",
    summary:
      "Aseprite’s official macOS build route provides precompiled Skia packages for arm64 and x86_64. The installer also requires the Apple compiler target to match the selected package.",
    steps: [
      {
        title: "Check this Mac",
        body: "Choose Apple menu › About This Mac and note whether the chip is Apple silicon or Intel.",
      },
      {
        title: "Use a matching installer build",
        body: "Quit this copy and open an arm64 installer on Apple silicon or an x86_64 installer on Intel.",
      },
      {
        title: "Check again",
        body: "No Mac restart is required after switching to the native installer.",
      },
    ],
    links: [
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  translation: {
    title: "Review Rosetta execution",
    summary:
      "This Intel installer is running through Rosetta on Apple silicon. This is a warning, not a blocker, when the x86_64 compiler and the full functional build test succeed.",
    steps: [
      {
        title: "Continue for an Intel build",
        body: "If every blocking check passes, the official script can build an x86_64 Aseprite app that runs through Rosetta.",
      },
      {
        title: "Prefer a native Apple silicon build",
        body: "Quit this installer and use its arm64 build if you want the resulting Aseprite app to run natively.",
      },
      {
        title: "Do not change Xcode blindly",
        body: "The separate compiler-target and C++17 checks decide whether this configuration is coherent. No reboot is required.",
      },
    ],
    links: [
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  xcode: {
    title: "Finish Apple developer tools setup",
    summary:
      "A working Apple compiler and macOS SDK are required. The installer tests them functionally and can use Command Line Tools or a valid Xcode found in /Applications.",
    steps: [
      {
        title: "Install developer tools",
        body: "Install Xcode from Apple or install Apple Command Line Tools from Terminal.",
        command: "xcode-select --install",
      },
      {
        title: "Finish first-launch setup",
        body: "If you installed full Xcode, open it once and let requested components and license prompts finish. The installer selects a working Xcode for its child processes without changing the system-wide selection.",
      },
      {
        title: "Verify the compiler",
        body: "These commands should report a developer directory and compiler. Then click Check again; no reboot is required.",
        command: "xcode-select -p\nxcrun --sdk macosx clang++ --version",
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
        title: "Complete the selected tools",
        body: "If the command fails, finish Xcode’s component or license prompts, or reinstall matching Apple Command Line Tools. The installer also scans valid Xcode apps in /Applications.",
      },
      {
        title: "Finish Xcode setup",
        body: "Open Xcode and let it install requested platform components, then click Check again. No reboot is required.",
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
      "Aseprite recommends the latest CMake, while the installer enforces the actual minimum declared by the selected verified source (currently 3.20). Homebrew is optional; the official universal macOS package is the manual alternative.",
    steps: [
      {
        title: "With Homebrew",
        body: "Install the current stable CMake formula.",
        command: "brew install cmake",
      },
      {
        title: "Without Homebrew",
        body: "Download the universal macOS .dmg from cmake.org and install CMake.app in /Applications or your user Applications folder. The installer detects its bundled command directly.",
      },
      {
        title: "Verify",
        body: "The selected source release determines the minimum accepted CMake version. Click Check again after installation; no app or Mac restart is required.",
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
      "Aseprite uses Ninja as its build system. Install the Homebrew formula or the official macOS release binary; the installer checks standard package-manager locations and ~/.local/bin directly.",
    steps: [
      {
        title: "With Homebrew",
        body: "Install the current stable Ninja formula.",
        command: "brew install ninja",
      },
      {
        title: "Without Homebrew",
        body: "Download the macOS archive from Ninja’s official GitHub Releases page, extract it, and install it for only your user. The installer scans ~/.local/bin directly.",
        command: "mkdir -p ~/.local/bin\ninstall -m 0755 ./ninja ~/.local/bin/ninja",
      },
      {
        title: "Verify",
        body: "Click Check again after installation; no app or Mac restart is required.",
        command: "ninja --version",
      },
    ],
    links: [
      { label: "Official Ninja releases", url: "https://github.com/ninja-build/ninja/releases" },
      { label: "Homebrew Ninja formula", url: "https://formulae.brew.sh/formula/ninja" },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  curl: {
    title: "Restore the macOS HTTPS client",
    summary:
      "Aseprite’s official build.sh downloads its matching precompiled Skia package with /usr/bin/curl. The installer verifies the exact TLS, failure, retry, and timeout options it wraps around that download.",
    steps: [
      {
        title: "Do not bypass TLS",
        body: "Do not use an insecure curl flag or disable certificate checks. On a managed network, verify the macOS proxy and corporate certificate trust with IT.",
      },
      {
        title: "Restore the system tool",
        body: "If /usr/bin/curl is missing or unusable, install available macOS updates or ask an administrator to repair the system installation.",
      },
      {
        title: "Check again",
        body: "No restart is normally required unless Software Update explicitly asks for one.",
      },
    ],
    links: [
      {
        label: "Apple: Change proxy settings",
        url: "https://support.apple.com/guide/mac-help/change-proxy-settings-on-mac-mchlp2591/mac",
      },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  unzip: {
    title: "Restore the macOS ZIP extractor",
    summary:
      "The official build.sh uses /usr/bin/unzip to extract the downloaded Skia package after curl succeeds.",
    steps: [
      {
        title: "Restore the system tool",
        body: "If /usr/bin/unzip is missing or unusable, install available macOS updates or ask an administrator to repair the system installation.",
      },
      {
        title: "Check again",
        body: "The installer executes unzip’s version probe again. No restart is normally required unless a system update requests it.",
      },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  skiaProxy: {
    title: "Restore the Skia HTTPS/CDN route",
    summary:
      "Release metadata and source archives use the app HTTP client, while Aseprite’s official build.sh invokes command-line curl for Skia. The installer separately performs a one-byte GET through GitHub’s real release redirect and CDN using that curl route.",
    steps: [
      {
        title: "Check the reported proxy mode",
        body: "Direct access, explicit HTTPS_PROXY/ALL_PROXY, static macOS HTTP/HTTPS, and static SOCKS settings are tested. PAC/WPAD, authenticated proxies, malformed variables, and managed routes must still complete the real curl probe.",
      },
      {
        title: "Allow every GitHub download host",
        body: "The network must allow api.github.com, github.com, and GitHub’s redirected release-assets.githubusercontent.com/CDN hosts with trusted TLS. Do not disable certificate verification.",
      },
      {
        title: "Retry after changing the route",
        body: "The installer refreshes proxy and Keychain trust settings for each operation and again before the build. No Mac restart is required.",
      },
    ],
    links: [
      {
        label: "Apple: Change proxy settings",
        url: "https://support.apple.com/guide/mac-help/change-proxy-settings-on-mac-mchlp2591/mac",
      },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  buildPath: {
    title: "Use a build path without whitespace",
    summary:
      "Aseprite’s official build.sh still expands some workspace paths without shell quoting. A space, tab, or line break in the installer cache path can redirect or break the build even when permissions are correct.",
    steps: [
      {
        title: "Read the detected path",
        body: "The failed requirement shows the exact build folder. This check is separate from permissions: making that folder writable does not make whitespace safe for the upstream script.",
      },
      {
        title: "Use a whitespace-free user cache",
        body: "Run the installer from a macOS account whose home and Library/Caches path contain no whitespace. For a managed or relocated home folder, ask your administrator or IT team for a compatible user-cache location rather than running the app with sudo.",
      },
      {
        title: "Check again",
        body: "Return to the installer after the cache path changes. No Mac restart is required.",
      },
    ],
    links: [
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  caseSensitiveBuild: {
    title: "Use a case-insensitive build volume",
    summary:
      "Aseprite’s current macOS build refers to both Aseprite.app and aseprite.app. They resolve to one bundle on the normal case-insensitive macOS format, but split into incomplete bundles on a case-sensitive volume.",
    steps: [
      {
        title: "Confirm the affected cache volume",
        body: "The requirement runs a temporary mixed-case name probe inside the real build folder and removes it immediately. The reported path identifies the volume that must change.",
      },
      {
        title: "Move to a case-insensitive volume",
        body: "Use a macOS account whose Library/Caches folder is on APFS or Mac OS Extended without the Case-sensitive option. If the home folder is managed or stored externally, ask your administrator or IT team to relocate the installer cache safely.",
      },
      {
        title: "Do not reformat without a backup",
        body: "Changing a volume format can erase data. Use Disk Utility only with a verified backup, then run the requirements check again; no restart is otherwise required.",
      },
    ],
    links: [
      {
        label: "Apple: File system formats",
        url: "https://support.apple.com/guide/disk-utility/file-system-formats-dsku19ed921c/mac",
      },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  baseline: {
    title: "Review the documented baseline",
    summary:
      "Aseprite currently documents Xcode 16.3 and macOS SDK 15.4 as a configuration known to work, while explicitly allowing that older or newer versions may work too.",
    steps: [
      {
        title: "Treat this as a warning",
        body: "A different version is not blocked by itself. The required C++17 build test below is the authoritative functional check.",
      },
      {
        title: "Continue when the functional checks pass",
        body: "If every blocking requirement is green, no downgrade, reboot, or system-wide Xcode switch is needed.",
      },
      {
        title: "Act only after a real failure",
        body: "If the compiler test fails, use its exact diagnostic to repair Xcode components, the SDK, CMake, or Ninja before checking again.",
      },
    ],
    links: [
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  workspace: {
    title: "Restore installer storage access",
    summary:
      "Each installer folder is tested only for the operations used there: durable fsync state/archive writes, executable build output with symbolic links and extended attributes, collision-safe archive/backup moves, atomic backup swaps, deletion, and the real registry/operation locks.",
    steps: [
      {
        title: "Read the detected path",
        body: "Use the exact failing folder shown by the requirement. A folder can exist yet still be unwritable because of ownership, ACLs, flags, a read-only volume, or a managed-device policy.",
      },
      {
        title: "Restore access for this account",
        body: "Move the installer data back to a writable user volume or correct its ownership using an administrator or your IT team. Do not run the installer itself with sudo and do not disable SIP or Gatekeeper.",
      },
      {
        title: "Check again",
        body: "The installer repeats the full mutation and bundle-metadata probe. No reboot is required.",
      },
    ],
    links: [
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  destination: {
    title: "Choose a writable destination",
    summary:
      "The destination parent must allow real file creation, execution, collision-safe rename, and atomic directory exchange while preserving symbolic links and extended attributes. Permission bits alone do not cover ACLs, noexec/read-only volumes, ownership, or macOS management policies.",
    steps: [
      {
        title: "Prefer your user Applications folder",
        body: "For a personal build, ~/Applications normally avoids administrator access. The installer creates that folder when the account can write to the home directory.",
      },
      {
        title: "Do not force a protected replacement",
        body: "If an existing copy belongs to Steam, a package manager, another user, or a managed /Applications location, keep it under its original channel and install a separate personal copy.",
      },
      {
        title: "Check again",
        body: "After changing the destination or its permissions, rerun the probe. No reboot or Full Disk Access grant is normally required.",
      },
    ],
    links: [
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  targetState: {
    title: "Make the existing app replaceable",
    summary:
      "macOS can deny a rename even when the parent folder is writable. The installer checks the selected bundle recursively for Locked, immutable, append, no-unlink, and system-restricted flags before compiling.",
    steps: [
      {
        title: "Use the reported item",
        body: "The requirement identifies the exact bundle item and flags that block a safe replacement.",
      },
      {
        title: "Remove only the relevant protection",
        body: "For a normal Locked flag, use Finder › Get Info. For system flags or a managed-device policy, ask an administrator or IT. Do not disable SIP or Gatekeeper and do not run this installer with sudo.",
      },
      {
        title: "Check again",
        body: "The installer repeats the recursive inspection immediately. A Mac restart is not required.",
      },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  asepriteClosed: {
    title: "Quit the selected Aseprite copy",
    summary:
      "Only the executable inside the app selected for replacement is checked. Another independent Steam or manual copy may remain open.",
    steps: [
      {
        title: "Save your work",
        body: "Save open sprites in the selected Aseprite copy, then choose Aseprite › Quit Aseprite.",
      },
      {
        title: "Check again",
        body: "The installer reads the real executable path of matching processes. You do not need to restart the Mac.",
      },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  disk: {
    title: "Free the build safety budget",
    summary:
      "The installer reserves a conservative 6 GB safety budget on its cache volume. Aseprite does not publish this as a minimum; measured builds use less, but peak usage can vary by release and toolchain.",
    steps: [
      {
        title: "Review storage",
        body: "Choose Apple menu › System Settings › General › Storage to see available space and Apple’s recommendations.",
      },
      {
        title: "Make the 6 GB budget available",
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
  toolchain: {
    title: "Repair the functional build chain",
    summary:
      "This check makes CMake configure a tiny Ninja project, then compiles, links, and runs a C++17 executable with the exact Apple tools selected for the real Aseprite build.",
    steps: [
      {
        title: "Use the detected error",
        body: "The requirement detail identifies the failing configure, compiler, linker, SDK, Ninja, or executable step. Repair that specific component instead of reinstalling everything blindly.",
      },
      {
        title: "Finish Apple setup",
        body: "Open Xcode if it has pending license or component prompts. Verify CMake and Ninja separately when their checks are not green.",
      },
      {
        title: "Check again",
        body: "The probe uses a clean temporary build directory and sanitized build variables, so no cache purge or reboot is required.",
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
};

export function getPrerequisiteGuide(id: string): PrerequisiteGuide {
  return guides[id] ?? {
    title: "Resolve this requirement",
    summary: "Follow the requirement details below, then run the check again.",
    steps: [],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  };
}
