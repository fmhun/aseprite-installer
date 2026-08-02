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

export type GuidePlatform = "macos" | "windows" | "linux";

const ASEPRITE_INSTALL_URL =
  "https://github.com/aseprite/aseprite/blob/35c35e645f68b6a2d39808c9e7b193d3144f100d/INSTALL.md";

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

const WINDOWS_CPP_DOCS =
  "https://learn.microsoft.com/en-us/cpp/build/vscpp-step-0-installation?view=msvc-170";
const WINDOWS_SDK_DOCS =
  "https://developer.microsoft.com/en-us/windows/downloads/windows-sdk/";

const windowsBuildToolsGuide: PrerequisiteGuide = {
  title: "Install the Windows C++ build tools",
  summary:
    "Aseprite needs a native Visual Studio 2022 x64 toolchain, Windows SDK 10.0.26100.0, CMake, and Ninja. The installer never changes Visual Studio or requests elevation; complete these steps yourself, then check again.",
  steps: [
    {
      title: "Add the Visual Studio C++ workload",
      body: "Open Visual Studio Installer, choose Modify for Visual Studio 2022 or Build Tools 2022, then select Desktop development with C++. In Individual components, keep the latest MSVC v143 x64/x86 tools and Windows 11 SDK 10.0.26100.0 selected.",
    },
    {
      title: "Install CMake and Ninja",
      body: "Use the CMake tools included by the Visual Studio workload, or run these winget commands yourself in PowerShell. Aseprite Installer only displays these commands; it never executes them or changes Visual Studio.",
      command:
        "winget install --exact --id Kitware.CMake\nwinget install --exact --id Ninja-build.Ninja",
    },
    {
      title: "Reopen and verify",
      body: "Close and reopen Aseprite Installer so it can discover the updated Visual Studio environment, then run Check again. A Windows restart is normally unnecessary.",
      command: "cmake --version\nninja --version",
    },
  ],
  links: [
    { label: "Microsoft: Install C++ support", url: WINDOWS_CPP_DOCS },
    { label: "Microsoft: Windows SDK", url: WINDOWS_SDK_DOCS },
    { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
  ],
};

const windowsDiskGuide: PrerequisiteGuide = {
  title: "Free the build safety budget",
  summary:
    "The installer reserves a conservative 6 GB safety budget on the volume containing its local build cache.",
  steps: [
    {
      title: "Review storage",
      body: "Open Settings › System › Storage and inspect the drive containing the reported local app-data path.",
    },
    {
      title: "Free the reported amount",
      body: "Review temporary files and large content before removing anything. Aseprite Installer never deletes unrelated files or empties the Recycle Bin.",
    },
    {
      title: "Check again",
      body: "Return after enough space is available; no Windows restart is required.",
    },
  ],
  links: [
    {
      label: "Microsoft: Free up drive space",
      url: "https://support.microsoft.com/en-us/windows/free-up-drive-space-in-windows-85529ccb-c365-490d-b548-831022bc9b32",
    },
  ],
};

const windowsGuides: Record<string, PrerequisiteGuide> = {
  nonElevated: {
    title: "Run the installer as a standard user",
    summary:
      "Aseprite Installer is per-user software and must not run with an administrator token. It never needs Run as administrator.",
    steps: [
      {
        title: "Close the elevated copy",
        body: "Quit this window. Do not change folder ownership or disable Windows security controls.",
      },
      {
        title: "Open it normally",
        body: "Launch Aseprite Installer from the Start menu or File Explorer without choosing Run as administrator.",
      },
      {
        title: "Check again",
        body: "Return to Requirements and rerun the checks. No Windows restart is required.",
      },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  osVersion: {
    title: "Use Windows 11 x64",
    summary:
      "The supported native build path requires Windows 11 x64 (build 22000 or newer). WSL and Windows-on-ARM emulation are not supported build environments.",
    steps: [
      {
        title: "Check the Windows version",
        body: "Open Settings › System › About and review Windows specifications and System type.",
      },
      {
        title: "Install supported updates",
        body: "Use Settings › Windows Update, then return here after any requested restart.",
      },
    ],
    links: [
      {
        label: "Microsoft: Windows 11 requirements",
        url: "https://support.microsoft.com/en-us/windows/windows-11-system-requirements-86c11283-ea52-4782-9efd-7674389a7ba3",
      },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  architecture: {
    title: "Use the native Windows x64 installer",
    summary:
      "Aseprite’s supported Windows Skia package and this installer’s build process target native x86_64 only.",
    steps: [
      {
        title: "Check System type",
        body: "Open Settings › System › About. Use the x64 Aseprite Installer on an x64-based Windows 11 computer.",
      },
      {
        title: "Avoid emulated build environments",
        body: "Do not build through WSL, an ARM emulation layer, or a cross compiler; use a native Windows x64 session.",
      },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  visualStudio: windowsBuildToolsGuide,
  cmake: windowsBuildToolsGuide,
  ninja: windowsBuildToolsGuide,
  toolchain: windowsBuildToolsGuide,
  workspace: {
    title: "Use a local writable build folder",
    summary:
      "The workspace and destination must support local file creation, execution, and atomic renames. Network and UNC build folders are not supported.",
    steps: [
      {
        title: "Keep installer data local",
        body: "Use the default per-user local app-data and installation folders. Avoid a network share, redirected UNC folder, or read-only location.",
      },
      {
        title: "Restore exact permissions",
        body: "If the reported path is blocked, restore write access for your user or ask your administrator. Do not reopen the installer as administrator.",
      },
      { title: "Check again", body: "Rerun the requirement check after fixing the reported path." },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  destination: {
    title: "Choose a writable per-user destination",
    summary:
      "The destination must be local and writable by your standard Windows account so replacement and rollback stay transactional.",
    steps: [
      {
        title: "Use the default per-user folder",
        body: "Prefer the location shown by Aseprite Installer. Keep Steam, package-manager, and other users’ copies under their original owner.",
      },
      {
        title: "Check Windows Security",
        body: "If Controlled Folder Access blocks the exact location, review the reported event and allow only the exact installer executable whose SHA-256 checksum and GitHub provenance you verified, if your policy permits it; never disable protection globally.",
      },
      { title: "Check again", body: "Rerun the requirement check; no restart is normally required." },
    ],
    links: [
      {
        label: "Microsoft: Controlled folder access",
        url: "https://support.microsoft.com/en-us/windows/virus-and-threat-protection-in-the-windows-security-app-1362f4cd-d71a-b52a-0b66-c2820032b65e",
      },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  executableDestination: {
    title: "Allow programs in the destination",
    summary:
      "The selected local destination must permit Aseprite’s executable to run under your standard account.",
    steps: [
      {
        title: "Review the reported path",
        body: "Use a normal per-user local application folder. If organization policy blocks executables there, ask IT for an approved location.",
      },
      { title: "Check again", body: "Rerun the functional destination probe after the policy or path changes." },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  asepriteClosed: {
    title: "Close the selected Aseprite copy",
    summary:
      "Windows cannot safely replace an executable while the selected Aseprite installation is running.",
    steps: [
      { title: "Save your work", body: "Save every open sprite, then exit the selected Aseprite copy." },
      {
        title: "Check Task Manager",
        body: "If the check still fails, open Task Manager and close only the Aseprite process using the reported executable path.",
      },
      { title: "Check again", body: "No Windows restart is required." },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  disk: windowsDiskGuide,
  diskSpace: windowsDiskGuide,
};

const linuxPackagesGuide: PrerequisiteGuide = {
  title: "Install the Linux build requirements",
  summary:
    "Install Clang, the GNU C++ standard library toolchain, CMake, Ninja, and the X11/OpenGL/font development packages with your distribution’s package manager. These commands are guidance only: Aseprite Installer never runs sudo or modifies system packages.",
  steps: [
    {
      title: "Debian or Ubuntu",
      body: "Run this yourself in a terminal, review the package plan, and approve it through your normal administrator policy.",
      command:
        "sudo apt update\nsudo apt install clang g++ cmake ninja-build libx11-dev libxcursor-dev libxi-dev libxrandr-dev libgl1-mesa-dev libfontconfig1-dev",
    },
    {
      title: "Fedora or RHEL family",
      body: "Run this yourself; package availability can depend on the enabled distribution repositories.",
      command:
        "sudo dnf install clang gcc-c++ cmake ninja-build libX11-devel libXcursor-devel libXi-devel libXrandr-devel mesa-libGL-devel fontconfig-devel",
    },
    {
      title: "Arch Linux",
      body: "Run this yourself with the official repositories enabled.",
      command:
        "sudo pacman -S --needed clang gcc cmake ninja libx11 libxcursor libxi libxrandr mesa fontconfig",
    },
    {
      title: "openSUSE",
      body: "Run this yourself; let zypper resolve the matching GNU C++ and development-package versions for your release.",
      command:
        "sudo zypper install clang gcc-c++ cmake ninja libX11-devel libXcursor-devel libXi-devel libXrandr-devel Mesa-libGL-devel fontconfig-devel",
    },
    {
      title: "Verify and check again",
      body: "Return to Aseprite Installer after the package transaction finishes. It functionally configures, compiles, links, and runs a small C++17 test before enabling installation.",
      command: "clang++ --version\ncmake --version\nninja --version",
    },
  ],
  links: [
    { label: "Aseprite Linux build requirements", url: ASEPRITE_INSTALL_URL },
    { label: "CMake downloads", url: "https://cmake.org/download/" },
    { label: "Ninja releases", url: "https://github.com/ninja-build/ninja/releases" },
  ],
};

const linuxGuides: Record<string, PrerequisiteGuide> = {
  nonElevated: {
    title: "Run the installer as your normal user",
    summary:
      "Aseprite Installer installs only for the current user and must never run through sudo or as root.",
    steps: [
      {
        title: "Close the root copy",
        body: "Quit this window. Do not use sudo to launch the AppImage or installed application.",
      },
      {
        title: "Open it normally",
        body: "Start Aseprite Installer from your desktop launcher or user terminal without sudo.",
      },
      {
        title: "Repair earlier ownership if needed",
        body: "If the requirement reports root-owned files from an earlier launch, ask an administrator to restore ownership only for those exact paths, then check again.",
      },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  linux: {
    title: "Use a supported Linux desktop baseline",
    summary:
      "The packaged GUI targets x86_64 Linux desktops at least as recent as Ubuntu 22.04 or Debian 12, including a compatible WebKitGTK 4.1 runtime.",
    steps: [
      {
        title: "Check the distribution",
        body: "Review /etc/os-release and your desktop package updates. Fedora, Arch, and openSUSE rolling/current releases can work when the functional checks pass.",
        command: "cat /etc/os-release",
      },
      {
        title: "Update through your administrator policy",
        body: "Use your distribution’s normal supported upgrade path. Aseprite Installer does not run system-package or sudo commands automatically.",
      },
      {
        title: "Ubuntu 20.04",
        body: "The current Tauri GUI baseline cannot support Ubuntu 20.04 reliably. Follow the tracked shared-engine CLI feature request instead of forcing newer system libraries into an LTS installation.",
      },
    ],
    links: [
      { label: "Ubuntu 20.04 CLI feature request", url: "https://github.com/fmhun/aseprite-installer/issues/4" },
      { label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL },
    ],
  },
  architecture: {
    title: "Use native Linux x86_64",
    summary:
      "The verified upstream Linux Skia archive and packaged installer target native x86_64. Linux ARM and emulated build environments are not supported.",
    steps: [
      { title: "Check the machine", body: "This command must report x86_64.", command: "uname -m" },
      {
        title: "Use a native system",
        body: "Open the x86_64 installer on a native x86_64 Linux desktop, then check again.",
      },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  clang: linuxPackagesGuide,
  cmake: linuxPackagesGuide,
  ninja: linuxPackagesGuide,
  toolchain: linuxPackagesGuide,
  workspace: {
    title: "Use a writable local workspace",
    summary:
      "The build cache must support normal user writes, executable files, durable sync, and atomic renames on a local filesystem.",
    steps: [
      {
        title: "Review the reported path",
        body: "Avoid root-owned, read-only, noexec, network, FUSE, or sandbox-restricted build folders. Keep the default XDG user cache when possible.",
      },
      {
        title: "Restore exact access",
        body: "Correct only the reported path through your normal administrator policy. Do not launch Aseprite Installer with sudo.",
      },
      { title: "Check again", body: "Rerun the full workspace probe after the filesystem or permissions change." },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  destination: {
    title: "Choose a writable executable destination",
    summary:
      "The installation folder must be local, user-writable, executable, and able to perform collision-safe atomic renames for rollback.",
    steps: [
      {
        title: "Prefer the displayed XDG user location",
        body: "Keep personal builds under the default user data location. Leave system, Flatpak, Snap, Steam, and package-manager copies under their original channel.",
      },
      {
        title: "Check mount options",
        body: "If the exact path is on a noexec or read-only mount, choose a normal local user filesystem rather than changing a system-wide mount casually.",
      },
      { title: "Check again", body: "Rerun the destination probe; no reboot is normally required." },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  executableDestination: {
    title: "Allow executables in the destination",
    summary:
      "The selected destination is writable but does not currently permit the functional executable probe to run.",
    steps: [
      {
        title: "Use a local executable user folder",
        body: "Choose the default XDG user installation location on a normal local filesystem. Do not disable SELinux or AppArmor globally.",
      },
      {
        title: "Review policy safely",
        body: "If a security policy blocks the reported probe, ask your administrator to approve the exact application path.",
      },
      { title: "Check again", body: "Rerun the probe after changing the path or policy." },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  asepriteClosed: {
    title: "Close the selected Aseprite copy",
    summary:
      "Save your work and close only the Aseprite executable selected for replacement before the transactional install begins.",
    steps: [
      { title: "Save and quit", body: "Save open sprites, then exit the selected Aseprite process normally." },
      {
        title: "Check the reported process",
        body: "If it remains detected, use your desktop system monitor to close only the process whose executable matches the reported installation path.",
      },
      { title: "Check again", body: "No session restart is normally required." },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  },
  disk: {
    ...guides.disk,
    steps: [
      {
        title: "Review free space",
        body: "Check the filesystem containing the displayed XDG cache path and free at least the reported safety budget.",
        command: "df -h",
      },
      {
        title: "Remove files deliberately",
        body: "Use your desktop storage tool or package manager to review content. Aseprite Installer never deletes unrelated files or empties the Trash.",
      },
      { title: "Check again", body: "Rerun the requirement check after space is available." },
    ],
  },
};

export function getPrerequisiteGuide(
  id: string,
  platform: GuidePlatform = "macos",
): PrerequisiteGuide {
  const platformGuides =
    platform === "windows"
      ? windowsGuides
      : platform === "linux"
        ? linuxGuides
        : guides;
  return platformGuides[id] ?? {
    title: "Resolve this requirement",
    summary:
      "Follow the detected remediation for this platform, make any system change yourself, then run the check again.",
    steps: [
      {
        title: "Use the detected issue",
        body: "Review the exact requirement detail below. Aseprite Installer does not elevate itself or modify system packages automatically.",
      },
    ],
    links: [{ label: "Aseprite build requirements", url: ASEPRITE_INSTALL_URL }],
  };
}
