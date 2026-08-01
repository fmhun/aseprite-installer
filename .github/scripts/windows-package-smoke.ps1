[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string]$BundleRoot,

  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string]$BuiltExecutable,

  [Parameter(Mandatory = $true)]
  [guid]$ExpectedUpgradeCode,

  [string]$OutputDirectory,

  [string]$StableSuffix
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not ('AsepriteInstaller.PackageSmoke.NativeWindow' -as [type])) {
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace AsepriteInstaller.PackageSmoke
{
    public static class NativeWindow
    {
        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool IsWindowVisible(IntPtr windowHandle);
    }

    public static class NativeResource
    {
        private const uint LoadLibraryAsDataFile = 0x00000002;
        private static readonly IntPtr ManifestResourceType = new IntPtr(24);
        private static readonly IntPtr PrimaryManifestResource = new IntPtr(1);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern IntPtr LoadLibraryExW(string fileName, IntPtr file, uint flags);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr FindResourceW(IntPtr module, IntPtr name, IntPtr type);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr LoadResource(IntPtr module, IntPtr resourceInfo);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr LockResource(IntPtr resourceData);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern uint SizeofResource(IntPtr module, IntPtr resourceInfo);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool FreeLibrary(IntPtr module);

        public static byte[] ReadPrimaryManifest(string path)
        {
            IntPtr module = LoadLibraryExW(path, IntPtr.Zero, LoadLibraryAsDataFile);
            if (module == IntPtr.Zero)
            {
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "Could not load the PE image as data.");
            }

            try
            {
                IntPtr resourceInfo = FindResourceW(module, PrimaryManifestResource, ManifestResourceType);
                if (resourceInfo == IntPtr.Zero)
                {
                    throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "The PE image has no primary RT_MANIFEST resource.");
                }
                uint size = SizeofResource(module, resourceInfo);
                IntPtr resourceData = LoadResource(module, resourceInfo);
                IntPtr bytes = LockResource(resourceData);
                if (size == 0 || resourceData == IntPtr.Zero || bytes == IntPtr.Zero)
                {
                    throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "Could not read the primary RT_MANIFEST resource.");
                }

                int byteCount = checked((int)size);
                byte[] result = new byte[byteCount];
                Marshal.Copy(bytes, result, 0, byteCount);
                return result;
            }
            finally
            {
                FreeLibrary(module);
            }
        }
    }
}
'@
}

function Fail {
  param([Parameter(Mandatory = $true)][string]$Message)
  throw "Windows package verification failed: $Message"
}

function Release-ComObject {
  param([object]$Value)

  if ($null -ne $Value -and [Runtime.InteropServices.Marshal]::IsComObject($Value)) {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($Value)
  }
}

function Test-KnownMsiAbsenceException {
  param([Parameter(Mandatory = $true)][Runtime.InteropServices.COMException]$Exception)

  # Windows Installer reports an absent product/component through
  # HRESULT_FROM_WIN32(ERROR_UNKNOWN_PRODUCT/ERROR_UNKNOWN_COMPONENT).
  # Do not turn service, access, or other COM failures into false "absent" results.
  $win32Code = ([int]$Exception.HResult -band 0xffff)
  return $win32Code -eq 1605 -or $win32Code -eq 1607
}

function Invoke-BoundedProcess {
  param(
    [Parameter(Mandatory = $true)][string]$FilePath,
    [Parameter(Mandatory = $true)][string[]]$ArgumentList,
    [Parameter(Mandatory = $true)][string]$Description,
    [int]$TimeoutSeconds = 180,
    [int[]]$AllowedExitCodes = @(0)
  )

  $startInfo = [Diagnostics.ProcessStartInfo]::new()
  $startInfo.FileName = $FilePath
  $startInfo.UseShellExecute = $false
  $startInfo.CreateNoWindow = $true
  foreach ($argument in $ArgumentList) {
    [void]$startInfo.ArgumentList.Add($argument)
  }

  $process = [Diagnostics.Process]::new()
  $process.StartInfo = $startInfo
  try {
    if (-not $process.Start()) {
      Fail "$Description did not start."
    }
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
      try {
        $process.Kill($true)
        [void]$process.WaitForExit(5000)
      } catch {
        Write-Warning "Could not terminate timed-out process $($process.Id): $($_.Exception.Message)"
      }
      Fail "$Description exceeded the ${TimeoutSeconds}-second timeout."
    }

    $exitCode = $process.ExitCode
    if ($AllowedExitCodes -notcontains $exitCode) {
      Fail "$Description exited with code $exitCode."
    }
    return $exitCode
  } finally {
    $process.Dispose()
  }
}

function Invoke-MsiProcess {
  param(
    [Parameter(Mandatory = $true)][string]$MsiExec,
    [Parameter(Mandatory = $true)][string[]]$ArgumentList,
    [Parameter(Mandatory = $true)][string]$Description,
    [Parameter(Mandatory = $true)][string]$LogPath
  )

  try {
    Invoke-BoundedProcess $MsiExec $ArgumentList $Description
  } catch {
    if (Test-Path -LiteralPath $LogPath -PathType Leaf) {
      Write-Warning "$Description failed. Last 120 lines from $LogPath follow."
      Get-Content -LiteralPath $LogPath -Tail 120 | ForEach-Object { Write-Host $_ }
    }
    throw
  }
}

function Assert-PeX64 {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Description
  )

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    Fail "$Description is missing: $Path"
  }

  $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
  $reader = $null
  try {
    $reader = [IO.BinaryReader]::new($stream)
    if ($stream.Length -lt 0x100 -or $reader.ReadUInt16() -ne 0x5a4d) {
      Fail "$Description is not a valid PE executable."
    }

    $stream.Position = 0x3c
    $peOffset = $reader.ReadUInt32()
    if ($peOffset -gt ($stream.Length - 26)) {
      Fail "$Description has an invalid PE header offset."
    }

    $stream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x00004550) {
      Fail "$Description has an invalid PE signature."
    }
    $machine = $reader.ReadUInt16()
    $stream.Position = $peOffset + 24
    $optionalHeaderMagic = $reader.ReadUInt16()
    if ($machine -ne 0x8664 -or $optionalHeaderMagic -ne 0x020b) {
      Fail ("$Description must be an x64 PE32+ executable; found machine 0x{0:x4}, optional header 0x{1:x4}." -f $machine, $optionalHeaderMagic)
    }
  } finally {
    if ($null -ne $reader) {
      $reader.Dispose()
    } else {
      $stream.Dispose()
    }
  }
}

function Assert-AsInvokerManifest {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Description
  )

  try {
    $manifestBytes = [AsepriteInstaller.PackageSmoke.NativeResource]::ReadPrimaryManifest($Path)
    if ($manifestBytes.Length -ge 2 -and $manifestBytes[0] -eq 0xff -and $manifestBytes[1] -eq 0xfe) {
      $manifestText = [Text.Encoding]::Unicode.GetString($manifestBytes)
    } elseif ($manifestBytes.Length -ge 2 -and $manifestBytes[0] -eq 0xfe -and $manifestBytes[1] -eq 0xff) {
      $manifestText = [Text.Encoding]::BigEndianUnicode.GetString($manifestBytes)
    } else {
      $manifestText = [Text.Encoding]::UTF8.GetString($manifestBytes)
    }

    $document = [Xml.XmlDocument]::new()
    $document.XmlResolver = $null
    $document.LoadXml($manifestText.Trim([char]0, [char]0xfeff))
    $executionLevels = @($document.SelectNodes("//*[local-name()='requestedExecutionLevel']"))
    if ($executionLevels.Count -ne 1) {
      Fail "$Description must contain exactly one requestedExecutionLevel element; found $($executionLevels.Count)."
    }
    $level = [string]$executionLevels[0].GetAttribute('level')
    $uiAccess = [string]$executionLevels[0].GetAttribute('uiAccess')
    if ($level -cne 'asInvoker' -or ($uiAccess -and $uiAccess -cne 'false')) {
      Fail "$Description must request level=asInvoker and uiAccess=false; found level='$level', uiAccess='$uiAccess'."
    }
  } catch {
    if ($_.Exception.Message -like 'Windows package verification failed:*') {
      throw
    }
    Fail "could not inspect the embedded manifest for $Description`: $($_.Exception.Message)"
  }
}

function Test-PathWithinRoot {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root
  )

  $fullPath = [IO.Path]::GetFullPath($Path).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
  $fullRoot = [IO.Path]::GetFullPath($Root).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
  return $fullPath.StartsWith("$fullRoot$([IO.Path]::DirectorySeparatorChar)", [StringComparison]::OrdinalIgnoreCase)
}

function Test-ForbiddenPayloadName {
  param([Parameter(Mandatory = $true)][string]$Name)

  $lower = $Name.ToLowerInvariant()
  return (
    $lower -eq 'aseprite' -or
    $lower -eq 'aseprite.exe' -or
    $lower -eq 'aseprite.app' -or
    $lower -eq 'skia.dll' -or
    $lower -eq 'skia.lib' -or
    $lower -eq 'icudtl.dat' -or
    $lower -like 'libskia*' -or
    $lower -like 'aseprite-v*-source.zip' -or
    $lower -like 'skia-*-release-*.zip'
  )
}

function Assert-NoForbiddenPayload {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$Description
  )

  if (-not (Test-Path -LiteralPath $Root -PathType Container)) {
    Fail "$Description does not exist: $Root"
  }

  $rootAttributes = [IO.File]::GetAttributes($Root)
  if (($rootAttributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    Fail "$Description root is a reparse point: $Root"
  }

  # Enumerate manually and never push a reparse-point directory onto the stack.
  # This prevents a packaged junction or symlink from escaping the extraction root.
  $directories = [Collections.Generic.Stack[string]]::new()
  $directories.Push($Root)
  while ($directories.Count -ne 0) {
    $directory = $directories.Pop()
    foreach ($entry in [IO.Directory]::EnumerateFileSystemEntries($directory)) {
      $attributes = [IO.File]::GetAttributes($entry)
      $relative = [IO.Path]::GetRelativePath($Root, $entry)
      if (($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        Fail "$Description contains a reparse point: $relative"
      }
      if (Test-ForbiddenPayloadName ([IO.Path]::GetFileName($entry))) {
        Fail "$Description contains forbidden Aseprite or Skia payload: $relative"
      }
      if (($attributes -band [IO.FileAttributes]::Directory) -ne 0) {
        $directories.Push($entry)
      }
    }
  }
}

function Get-MsiSingleValue {
  param(
    [Parameter(Mandatory = $true)][object]$Database,
    [Parameter(Mandatory = $true)][string]$Query,
    [Parameter(Mandatory = $true)][string]$Description
  )

  $view = $null
  $record = $null
  $extraRecord = $null
  try {
    $view = $Database.OpenView($Query)
    [void]$view.Execute()
    $record = $view.Fetch()
    if ($null -eq $record) {
      Fail "MSI metadata is missing $Description."
    }
    $value = [string]$record.StringData(1)
    $extraRecord = $view.Fetch()
    if ($null -ne $extraRecord) {
      Fail "MSI metadata contains more than one $Description value."
    }
    if ([string]::IsNullOrWhiteSpace($value)) {
      Fail "MSI metadata contains an empty $Description value."
    }
    return $value
  } finally {
    Release-ComObject $extraRecord
    Release-ComObject $record
    if ($null -ne $view) {
      try { [void]$view.Close() } catch { Write-Warning $_.Exception.Message }
    }
    Release-ComObject $view
  }
}

function Get-MsiMetadata {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$ExpectedExecutableName
  )

  $installer = $null
  $database = $null
  $summaryInformation = $null
  $view = $null
  $record = $null
  try {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $installer.OpenDatabase($Path, 0)
    $summaryInformation = $database.SummaryInformation(0)
    $template = [string]$summaryInformation.Property(7)
    if ([string]::IsNullOrWhiteSpace($template)) {
      Fail 'MSI SummaryInformation is missing template property 7.'
    }
    $templateParts = $template.Split([char[]]@(';'), 2, [StringSplitOptions]::None)
    if ($templateParts.Count -ne 2 -or
        [string]::IsNullOrWhiteSpace($templateParts[0]) -or
        [string]::IsNullOrWhiteSpace($templateParts[1])) {
      Fail "MSI SummaryInformation template property 7 is malformed: $template"
    }
    $architecture = $templateParts[0].Trim()
    $upgradeCode = Get-MsiSingleValue $database "SELECT Value FROM Property WHERE Property = 'UpgradeCode'" 'UpgradeCode'
    $productCode = Get-MsiSingleValue $database "SELECT Value FROM Property WHERE Property = 'ProductCode'" 'ProductCode'

    $componentKeys = [Collections.Generic.List[string]]::new()
    $view = $database.OpenView('SELECT `Component_`, `FileName` FROM `File`')
    [void]$view.Execute()
    while ($null -ne ($record = $view.Fetch())) {
      try {
        $msiFileName = [string]$record.StringData(2)
        $longFileName = ($msiFileName -split '\|')[-1]
        if ($longFileName -ieq $ExpectedExecutableName) {
          [void]$componentKeys.Add([string]$record.StringData(1))
        }
      } finally {
        Release-ComObject $record
        $record = $null
      }
    }
    if ($componentKeys.Count -ne 1) {
      Fail "MSI must contain exactly one $ExpectedExecutableName file row; found $($componentKeys.Count)."
    }

    $escapedComponent = $componentKeys[0].Replace("'", "''")
    $componentQuery = "SELECT ``ComponentId`` FROM ``Component`` WHERE ``Component`` = '$escapedComponent'"
    $componentId = Get-MsiSingleValue $database $componentQuery 'application ComponentId'

    return [pscustomobject]@{
      ProductCode = [guid]$productCode
      UpgradeCode = [guid]$upgradeCode
      ComponentId = [guid]$componentId
      Template = $template
      Architecture = $architecture
    }
  } finally {
    Release-ComObject $record
    if ($null -ne $view) {
      try { [void]$view.Close() } catch { Write-Warning $_.Exception.Message }
    }
    Release-ComObject $view
    Release-ComObject $summaryInformation
    Release-ComObject $database
    Release-ComObject $installer
  }
}

function Get-MsiComponentPath {
  param(
    [Parameter(Mandatory = $true)][guid]$ProductCode,
    [Parameter(Mandatory = $true)][guid]$ComponentId
  )

  $installer = $null
  try {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    return [string]$installer.ComponentPath($ProductCode.ToString('B'), $ComponentId.ToString('B'))
  } finally {
    Release-ComObject $installer
  }
}

function Get-MsiProductState {
  param([Parameter(Mandatory = $true)][guid]$ProductCode)

  $installer = $null
  try {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    return [int]$installer.ProductState($ProductCode.ToString('B'))
  } catch [Runtime.InteropServices.COMException] {
    if (Test-KnownMsiAbsenceException $_.Exception) {
      # INSTALLSTATE_UNKNOWN is the expected result once registration is gone.
      return -1
    }
    throw
  } finally {
    Release-ComObject $installer
  }
}

function Get-UninstallRegistryEntries {
  $entries = [Collections.Generic.List[object]]::new()
  $uninstallPath = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall'
  # HKCU\Software is shared by WOW64, so alternate views expose the same
  # physical uninstall keys twice. HKLM\Software is redirected and must be
  # inspected in both views.
  $scopes = @(
    [pscustomobject]@{
      Hive = [Microsoft.Win32.RegistryHive]::CurrentUser
      View = [Microsoft.Win32.RegistryView]::Default
    },
    [pscustomobject]@{
      Hive = [Microsoft.Win32.RegistryHive]::LocalMachine
      View = [Microsoft.Win32.RegistryView]::Registry64
    },
    [pscustomobject]@{
      Hive = [Microsoft.Win32.RegistryHive]::LocalMachine
      View = [Microsoft.Win32.RegistryView]::Registry32
    }
  )

  foreach ($scope in $scopes) {
    $hive = $scope.Hive
    $view = $scope.View
    $baseKey = $null
    $uninstallKey = $null
    try {
      $baseKey = [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, $view)
      $uninstallKey = $baseKey.OpenSubKey($uninstallPath, $false)
      if ($null -eq $uninstallKey) {
        continue
      }
      foreach ($keyName in $uninstallKey.GetSubKeyNames()) {
        $entryKey = $null
        try {
          $entryKey = $uninstallKey.OpenSubKey($keyName, $false)
          if ($null -eq $entryKey) {
            continue
          }
          $entries.Add([pscustomobject]@{
            Location = "$hive/$view/$keyName"
            Hive = [string]$hive
            View = [string]$view
            KeyName = [string]$keyName
            DisplayName = [string]$entryKey.GetValue('DisplayName', '')
            InstallLocation = [string]$entryKey.GetValue('InstallLocation', '')
            UninstallString = [string]$entryKey.GetValue('UninstallString', '')
            QuietUninstallString = [string]$entryKey.GetValue('QuietUninstallString', '')
          })
        } finally {
          if ($null -ne $entryKey) {
            $entryKey.Dispose()
          }
        }
      }
    } finally {
      if ($null -ne $uninstallKey) {
        $uninstallKey.Dispose()
      }
      if ($null -ne $baseKey) {
        $baseKey.Dispose()
      }
    }
  }

  return $entries
}

function Test-UninstallEntryRelevant {
  param(
    [Parameter(Mandatory = $true)][object]$Entry,
    [Parameter(Mandatory = $true)][string]$ExpectedExecutableName,
    [string]$InstallRoot,
    [guid]$ProductCode = [guid]::Empty
  )

  if ($Entry.DisplayName -like 'Aseprite Installer*') {
    return $true
  }
  if ($ProductCode -ne [guid]::Empty -and $Entry.KeyName -ieq $ProductCode.ToString('B')) {
    return $true
  }

  $combined = "$($Entry.InstallLocation)`n$($Entry.UninstallString)`n$($Entry.QuietUninstallString)"
  if ($combined.IndexOf($ExpectedExecutableName, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
    return $true
  }
  if ($InstallRoot -and $combined.IndexOf($InstallRoot, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
    return $true
  }
  return $false
}

function Get-RelevantUninstallEntries {
  param(
    [Parameter(Mandatory = $true)][string]$ExpectedExecutableName,
    [string]$InstallRoot,
    [guid]$ProductCode = [guid]::Empty
  )

  return @(Get-UninstallRegistryEntries | Where-Object {
    Test-UninstallEntryRelevant $_ $ExpectedExecutableName $InstallRoot $ProductCode
  })
}

function Format-RegistryLocations {
  param([Parameter(Mandatory = $true)][object[]]$Entries)
  return (($Entries | ForEach-Object { $_.Location }) -join ', ')
}

function Get-ShortcutEntries {
  $entries = [Collections.Generic.List[object]]::new()
  $roots = @(
    [pscustomobject]@{ Scope = 'CurrentUser'; Kind = 'Desktop'; Path = [Environment]::GetFolderPath([Environment+SpecialFolder]::DesktopDirectory) },
    [pscustomobject]@{ Scope = 'CurrentUser'; Kind = 'StartMenu'; Path = [Environment]::GetFolderPath([Environment+SpecialFolder]::StartMenu) },
    [pscustomobject]@{ Scope = 'LocalMachine'; Kind = 'Desktop'; Path = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonDesktopDirectory) },
    [pscustomobject]@{ Scope = 'LocalMachine'; Kind = 'StartMenu'; Path = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonStartMenu) }
  )

  foreach ($root in $roots) {
    if ([string]::IsNullOrWhiteSpace($root.Path) -or -not (Test-Path -LiteralPath $root.Path -PathType Container)) {
      continue
    }
    foreach ($file in (Get-ChildItem -LiteralPath $root.Path -Filter '*.lnk' -File -Recurse -ErrorAction Stop)) {
      $entries.Add([pscustomobject]@{
        Scope = $root.Scope
        Kind = $root.Kind
        Path = $file.FullName
      })
    }
  }
  return $entries
}

function Get-RelevantShortcutEntries {
  return @(Get-ShortcutEntries | Where-Object {
    $_.Path.IndexOf('Aseprite Installer', [StringComparison]::OrdinalIgnoreCase) -ge 0 -or
    $_.Path.IndexOf('aseprite-installer', [StringComparison]::OrdinalIgnoreCase) -ge 0
  })
}

function Format-ShortcutPaths {
  param([Parameter(Mandatory = $true)][object[]]$Entries)
  return (($Entries | ForEach-Object { $_.Path }) -join ', ')
}

function Get-ShortcutTargetPath {
  param([Parameter(Mandatory = $true)][string]$Path)

  $shell = $null
  $shortcut = $null
  try {
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($Path)
    $target = [Environment]::ExpandEnvironmentVariables([string]$shortcut.TargetPath)
    if ([string]::IsNullOrWhiteSpace($target) -or -not [IO.Path]::IsPathRooted($target)) {
      Fail "shortcut has an empty or non-absolute target: $Path"
    }
    if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
      Fail "shortcut target does not exist: $Path -> $target"
    }
    return (Resolve-Path -LiteralPath $target).ProviderPath
  } finally {
    Release-ComObject $shortcut
    Release-ComObject $shell
  }
}

function Get-RegisteredExecutablePath {
  param(
    [Parameter(Mandatory = $true)][AllowEmptyString()][string]$CommandLine,
    [Parameter(Mandatory = $true)][string]$Description
  )

  $trimmed = $CommandLine.Trim()
  if ([string]::IsNullOrWhiteSpace($trimmed)) {
    Fail "$Description is empty."
  }

  if ($trimmed.StartsWith('"', [StringComparison]::Ordinal)) {
    $closingQuote = $trimmed.IndexOf('"', 1)
    if ($closingQuote -lt 2) {
      Fail "$Description has malformed quoting: $CommandLine"
    }
    $executable = $trimmed.Substring(1, $closingQuote - 1)
    $remainder = $trimmed.Substring($closingQuote + 1)
    if ($remainder.Length -ne 0 -and -not [char]::IsWhiteSpace($remainder[0])) {
      Fail "$Description has ambiguous text after its quoted executable: $CommandLine"
    }
  } else {
    $firstWhitespace = $trimmed.IndexOfAny([char[]]@(' ', "`t"))
    if ($firstWhitespace -ge 0) {
      $executable = $trimmed.Substring(0, $firstWhitespace)
    } else {
      $executable = $trimmed
    }
  }

  $executable = [Environment]::ExpandEnvironmentVariables($executable)
  if (-not [IO.Path]::IsPathRooted($executable) -or
      -not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    Fail "$Description does not resolve to an existing absolute executable: $executable"
  }
  return (Resolve-Path -LiteralPath $executable).ProviderPath
}

function Wait-CapturedArtifactsAbsent {
  param(
    [Parameter(Mandatory = $true)][string[]]$RegistryLocations,
    [Parameter(Mandatory = $true)][string[]]$ShortcutPaths,
    [int]$TimeoutSeconds = 20
  )

  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  do {
    $currentRegistryLocations = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in @(Get-UninstallRegistryEntries)) {
      [void]$currentRegistryLocations.Add([string]$entry.Location)
    }
    $remainingRegistry = @($RegistryLocations | Where-Object { $currentRegistryLocations.Contains($_) })
    $remainingShortcuts = @($ShortcutPaths | Where-Object { Test-Path -LiteralPath $_ })
    if ($remainingRegistry.Count -eq 0 -and $remainingShortcuts.Count -eq 0) {
      return
    }
    Start-Sleep -Milliseconds 250
  } while ([DateTime]::UtcNow -lt $deadline)

  if ($remainingRegistry.Count -ne 0) {
    Fail "captured uninstall key remains after ${TimeoutSeconds} seconds: $($remainingRegistry -join ', ')"
  }
  Fail "captured shortcut remains after ${TimeoutSeconds} seconds: $($remainingShortcuts -join ', ')"
}

function Wait-RegistrationArtifactsAbsent {
  param(
    [Parameter(Mandatory = $true)][string]$ExpectedExecutableName,
    [string]$InstallRoot,
    [guid]$ProductCode = [guid]::Empty,
    [int]$TimeoutSeconds = 20
  )

  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  do {
    $registryEntries = @(Get-RelevantUninstallEntries $ExpectedExecutableName $InstallRoot $ProductCode)
    $shortcutEntries = @(Get-RelevantShortcutEntries)
    if ($registryEntries.Count -eq 0 -and $shortcutEntries.Count -eq 0) {
      return
    }
    Start-Sleep -Milliseconds 250
  } while ([DateTime]::UtcNow -lt $deadline)

  if ($registryEntries.Count -ne 0) {
    Fail "uninstall registration remains after ${TimeoutSeconds} seconds: $(Format-RegistryLocations $registryEntries)"
  }
  Fail "installer shortcut remains after ${TimeoutSeconds} seconds: $(Format-ShortcutPaths $shortcutEntries)"
}

function Invoke-AppAliveSmoke {
  param(
    [Parameter(Mandatory = $true)][string]$Executable,
    [Parameter(Mandatory = $true)][string]$PackageKind,
    [int]$WindowTimeoutSeconds = 30,
    [int]$AliveSeconds = 5
  )

  $process = $null
  try {
    $process = Start-Process -FilePath $Executable -PassThru
    $windowDeadline = [DateTime]::UtcNow.AddSeconds($WindowTimeoutSeconds)
    $observedTitle = ''
    $windowReady = $false
    while ([DateTime]::UtcNow -lt $windowDeadline) {
      Start-Sleep -Milliseconds 250
      $process.Refresh()
      if ($process.HasExited) {
        Fail "$PackageKind installed app exited before displaying its main window (code $($process.ExitCode))."
      }

      $windowHandle = $process.MainWindowHandle
      $observedTitle = $process.MainWindowTitle
      if ($windowHandle -ne [IntPtr]::Zero -and
          $observedTitle -ceq 'Aseprite Installer' -and
          [AsepriteInstaller.PackageSmoke.NativeWindow]::IsWindowVisible($windowHandle)) {
        $windowReady = $true
        break
      }
    }
    if (-not $windowReady) {
      Fail "$PackageKind installed app did not display a visible main window titled exactly 'Aseprite Installer' within ${WindowTimeoutSeconds} seconds (last observed title: '$observedTitle')."
    }

    $aliveDeadline = [DateTime]::UtcNow.AddSeconds($AliveSeconds)
    while ([DateTime]::UtcNow -lt $aliveDeadline) {
      Start-Sleep -Milliseconds 250
      $process.Refresh()
      if ($process.HasExited) {
        Fail "$PackageKind installed app exited during its ${AliveSeconds}-second post-window launch smoke (code $($process.ExitCode))."
      }
    }
  } finally {
    if ($null -ne $process) {
      try {
        $process.Refresh()
        if (-not $process.HasExited) {
          $process.Kill($true)
          if (-not $process.WaitForExit(10000)) {
            Fail "$PackageKind installed app did not stop after its launch smoke."
          }
        }
      } finally {
        $process.Dispose()
      }
    }
  }
}

function Wait-PathAbsent {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Description,
    [int]$TimeoutSeconds = 20
  )

  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  while ((Test-Path -LiteralPath $Path) -and [DateTime]::UtcNow -lt $deadline) {
    Start-Sleep -Milliseconds 250
  }
  if (Test-Path -LiteralPath $Path) {
    Fail "$Description still exists after ${TimeoutSeconds} seconds: $Path"
  }
}

function Wait-MsiComponentAbsent {
  param(
    [Parameter(Mandatory = $true)][guid]$ProductCode,
    [Parameter(Mandatory = $true)][guid]$ComponentId,
    [int]$TimeoutSeconds = 20
  )

  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  $componentPath = ''
  do {
    try {
      $componentPath = Get-MsiComponentPath $ProductCode $ComponentId
    } catch [Runtime.InteropServices.COMException] {
      if (Test-KnownMsiAbsenceException $_.Exception) {
        # Windows Installer may report an unknown product/component after removal.
        $componentPath = ''
      } else {
        throw
      }
    }
    if ([string]::IsNullOrWhiteSpace($componentPath)) {
      return
    }
    Start-Sleep -Milliseconds 250
  } while ([DateTime]::UtcNow -lt $deadline)

  Fail "Windows Installer still resolves the application component after ${TimeoutSeconds} seconds: $componentPath"
}

function Wait-MsiProductUnregistered {
  param(
    [Parameter(Mandatory = $true)][guid]$ProductCode,
    [int]$TimeoutSeconds = 20
  )

  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  $productState = Get-MsiProductState $ProductCode
  while ($productState -ne -1 -and [DateTime]::UtcNow -lt $deadline) {
    Start-Sleep -Milliseconds 250
    $productState = Get-MsiProductState $ProductCode
  }
  if ($productState -ne -1) {
    Fail "Windows Installer ProductState remains registered as $productState after ${TimeoutSeconds} seconds."
  }
}

if ([string]::IsNullOrWhiteSpace($OutputDirectory) -xor [string]::IsNullOrWhiteSpace($StableSuffix)) {
  Fail 'OutputDirectory and StableSuffix must either both be provided or both be omitted.'
}
if ($StableSuffix -and $StableSuffix -notmatch '^[A-Za-z0-9_-]+$') {
  Fail 'StableSuffix contains unsupported characters.'
}

$resolvedBundleRoot = (Resolve-Path -LiteralPath $BundleRoot).ProviderPath
$resolvedBuiltExecutable = (Resolve-Path -LiteralPath $BuiltExecutable).ProviderPath
$expectedExecutableName = [IO.Path]::GetFileName($resolvedBuiltExecutable)
$nsisDirectory = Join-Path $resolvedBundleRoot 'nsis'
$msiDirectory = Join-Path $resolvedBundleRoot 'msi'
if (-not (Test-Path -LiteralPath $nsisDirectory -PathType Container) -or
    -not (Test-Path -LiteralPath $msiDirectory -PathType Container)) {
  Fail 'bundle root must contain nsis and msi directories.'
}
$nsisPackages = @(Get-ChildItem -LiteralPath $nsisDirectory -Filter '*.exe' -File)
$msiPackages = @(Get-ChildItem -LiteralPath $msiDirectory -Filter '*.msi' -File)
if ($nsisPackages.Count -ne 1 -or $msiPackages.Count -ne 1) {
  Fail "expected exactly one NSIS and one MSI package; found NSIS=$($nsisPackages.Count), MSI=$($msiPackages.Count)."
}
$nsisPackage = $nsisPackages[0].FullName
$msiPackage = $msiPackages[0].FullName

Assert-PeX64 $resolvedBuiltExecutable 'unpackaged application executable'
Assert-AsInvokerManifest $resolvedBuiltExecutable 'unpackaged application executable'
Assert-AsInvokerManifest $nsisPackage 'NSIS installer'
$msiMetadata = Get-MsiMetadata $msiPackage $expectedExecutableName
if ($msiMetadata.Architecture -ine 'x64') {
  Fail "MSI SummaryInformation template must declare x64 architecture; found '$($msiMetadata.Template)'."
}
if ($msiMetadata.UpgradeCode -ne $ExpectedUpgradeCode) {
  Fail "unexpected MSI UpgradeCode $($msiMetadata.UpgradeCode); expected $ExpectedUpgradeCode."
}
$existingMsiComponentPath = try {
  Get-MsiComponentPath $msiMetadata.ProductCode $msiMetadata.ComponentId
} catch [Runtime.InteropServices.COMException] {
  if (Test-KnownMsiAbsenceException $_.Exception) {
    ''
  } else {
    throw
  }
}
if (-not [string]::IsNullOrWhiteSpace($existingMsiComponentPath)) {
  Fail "the MSI ProductCode is already installed on this runner: $existingMsiComponentPath"
}
$existingMsiProductState = Get-MsiProductState $msiMetadata.ProductCode
if ($existingMsiProductState -ne -1) {
  Fail "the MSI ProductCode is already registered on this runner with ProductState $existingMsiProductState."
}
$preexistingRegistrations = @(Get-RelevantUninstallEntries $expectedExecutableName '' $msiMetadata.ProductCode)
if ($preexistingRegistrations.Count -ne 0) {
  Fail "a relevant uninstall registration already exists on this runner: $(Format-RegistryLocations $preexistingRegistrations)"
}
$preexistingShortcuts = @(Get-RelevantShortcutEntries)
if ($preexistingShortcuts.Count -ne 0) {
  Fail "a relevant installer shortcut already exists on this runner: $(Format-ShortcutPaths $preexistingShortcuts)"
}

$temporaryBase = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$smokeIdentifier = [guid]::NewGuid().ToString('N')
$workRoot = Join-Path $temporaryBase "aseprite-installer-windows-smoke-$smokeIdentifier"
$msiExtractRoot = Join-Path $workRoot 'msi-extract'
$userProfile = [Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile)
$localAppData = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
if ([string]::IsNullOrWhiteSpace($userProfile) -or [string]::IsNullOrWhiteSpace($localAppData)) {
  Fail 'Windows did not provide the current user profile and local application-data paths.'
}
$nsisInstallRoot = Join-Path $localAppData "Programs\AsepriteInstallerPackageSmoke-$smokeIdentifier"
if (-not (Test-PathWithinRoot $localAppData $userProfile) -or
    -not (Test-PathWithinRoot $nsisInstallRoot $localAppData)) {
  Fail "the isolated NSIS destination is not confined to the current user profile: $nsisInstallRoot"
}
$msiExtractLog = Join-Path $workRoot 'msi-extract.log'
$msiInstallLog = Join-Path $workRoot 'msi-install.log'
$msiUninstallLog = Join-Path $workRoot 'msi-uninstall.log'
$msiInstalled = $false
$nsisInstalled = $false
$nsisUninstaller = $null
$msiInstallRoot = $null
$systemDirectory = [Environment]::GetFolderPath([Environment+SpecialFolder]::System)
$msiExec = Join-Path $systemDirectory 'msiexec.exe'

New-Item -ItemType Directory -Path $workRoot, $msiExtractRoot | Out-Null
try {
  Invoke-MsiProcess $msiExec @('/a', $msiPackage, '/qn', '/norestart', '/l*v', $msiExtractLog, "TARGETDIR=$msiExtractRoot") 'MSI administrative extraction' $msiExtractLog
  Assert-NoForbiddenPayload $msiExtractRoot 'MSI administrative image'

  # /D must remain the final NSIS argument.
  $nsisInstalled = $true
  Invoke-BoundedProcess $nsisPackage @('/S', "/D=$nsisInstallRoot") 'NSIS silent installation'
  Assert-NoForbiddenPayload $nsisInstallRoot 'NSIS installed tree'
  $nsisExecutables = @(Get-ChildItem -LiteralPath $nsisInstallRoot -Recurse -File | Where-Object {
    $_.Name -ieq $expectedExecutableName
  })
  if ($nsisExecutables.Count -ne 1) {
    Fail "NSIS install must contain exactly one $expectedExecutableName; found $($nsisExecutables.Count)."
  }
  $nsisExecutable = $nsisExecutables[0].FullName
  Assert-PeX64 $nsisExecutable 'NSIS installed application executable'
  Assert-AsInvokerManifest $nsisExecutable 'NSIS installed application executable'

  $nsisRegistrations = @(Get-RelevantUninstallEntries $expectedExecutableName $nsisInstallRoot)
  $machineNsisRegistrations = @($nsisRegistrations | Where-Object { $_.Hive -eq 'LocalMachine' })
  if ($machineNsisRegistrations.Count -ne 0) {
    Fail "NSIS created a machine uninstall registration: $(Format-RegistryLocations $machineNsisRegistrations)"
  }
  $currentUserNsisRegistrations = @($nsisRegistrations | Where-Object { $_.Hive -eq 'CurrentUser' })
  if ($currentUserNsisRegistrations.Count -ne 1) {
    Fail "NSIS must create exactly one current-user uninstall registration; found $($currentUserNsisRegistrations.Count)."
  }
  $nsisRegistration = $currentUserNsisRegistrations[0]
  $registrationPaths = "$($nsisRegistration.InstallLocation)`n$($nsisRegistration.UninstallString)`n$($nsisRegistration.QuietUninstallString)"
  if ($registrationPaths.IndexOf($nsisInstallRoot, [StringComparison]::OrdinalIgnoreCase) -lt 0) {
    Fail "NSIS uninstall registration does not reference its profile-confined installation root: $nsisInstallRoot"
  }
  $registeredNsisUninstaller = Get-RegisteredExecutablePath $nsisRegistration.UninstallString 'NSIS HKCU UninstallString'
  if (-not (Test-PathWithinRoot $registeredNsisUninstaller $nsisInstallRoot) -or
      [IO.Path]::GetFileName($registeredNsisUninstaller) -notlike 'uninstall*.exe') {
    Fail "NSIS HKCU UninstallString does not point to the expected uninstaller beneath its install root: $registeredNsisUninstaller"
  }
  $nsisUninstaller = $registeredNsisUninstaller
  Assert-AsInvokerManifest $nsisUninstaller 'registered NSIS uninstaller'

  $machineNsisShortcuts = @(Get-RelevantShortcutEntries | Where-Object { $_.Scope -eq 'LocalMachine' })
  if ($machineNsisShortcuts.Count -ne 0) {
    Fail "NSIS created a machine shortcut: $(Format-ShortcutPaths $machineNsisShortcuts)"
  }
  $nsisStartMenuShortcuts = @(Get-RelevantShortcutEntries | Where-Object {
    $_.Scope -eq 'CurrentUser' -and $_.Kind -eq 'StartMenu'
  })
  if ($nsisStartMenuShortcuts.Count -ne 1) {
    Fail "NSIS must create exactly one current-user Start menu shortcut; found $($nsisStartMenuShortcuts.Count)."
  }
  $nsisShortcutTarget = Get-ShortcutTargetPath $nsisStartMenuShortcuts[0].Path
  if (-not $nsisShortcutTarget.Equals($nsisExecutable, [StringComparison]::OrdinalIgnoreCase)) {
    Fail "NSIS Start menu shortcut targets '$nsisShortcutTarget', expected '$nsisExecutable'."
  }
  $capturedNsisRegistryLocations = @($currentUserNsisRegistrations | ForEach-Object { [string]$_.Location })
  $capturedNsisShortcutPaths = @($nsisStartMenuShortcuts | ForEach-Object { [string]$_.Path })

  Invoke-AppAliveSmoke $nsisExecutable 'NSIS'
  Invoke-BoundedProcess $nsisUninstaller @('/S') 'NSIS silent uninstallation'
  Wait-PathAbsent $nsisInstallRoot 'NSIS installation root'
  Wait-CapturedArtifactsAbsent $capturedNsisRegistryLocations $capturedNsisShortcutPaths
  Wait-RegistrationArtifactsAbsent $expectedExecutableName $nsisInstallRoot
  $nsisInstalled = $false

  $msiInstalled = $true
  Invoke-MsiProcess $msiExec @('/i', $msiPackage, '/qn', '/norestart', '/l*v', $msiInstallLog) 'MSI silent installation' $msiInstallLog
  $installedMsiProductState = Get-MsiProductState $msiMetadata.ProductCode
  if ($installedMsiProductState -eq -1) {
    Fail "MSI installation did not register the product; ProductState is $installedMsiProductState."
  }
  $msiRegistrations = @(Get-RelevantUninstallEntries $expectedExecutableName '' $msiMetadata.ProductCode)
  if ($msiRegistrations.Count -eq 0) {
    Fail 'MSI installation did not create a discoverable uninstall registration.'
  }
  $msiExecutable = Get-MsiComponentPath $msiMetadata.ProductCode $msiMetadata.ComponentId
  if ([string]::IsNullOrWhiteSpace($msiExecutable)) {
    Fail 'Windows Installer did not resolve the installed application component path.'
  }
  $msiExecutable = (Resolve-Path -LiteralPath $msiExecutable).ProviderPath
  $msiInstallRoot = [IO.Path]::GetDirectoryName($msiExecutable)
  Assert-PeX64 $msiExecutable 'MSI installed application executable'
  Assert-AsInvokerManifest $msiExecutable 'MSI installed application executable'
  Assert-NoForbiddenPayload $msiInstallRoot 'MSI installed application tree'
  $msiStartMenuShortcuts = @(Get-RelevantShortcutEntries | Where-Object { $_.Kind -eq 'StartMenu' })
  if ($msiStartMenuShortcuts.Count -ne 1) {
    Fail "MSI must create exactly one Start menu shortcut; found $($msiStartMenuShortcuts.Count)."
  }
  $msiShortcutTarget = Get-ShortcutTargetPath $msiStartMenuShortcuts[0].Path
  if (-not $msiShortcutTarget.Equals($msiExecutable, [StringComparison]::OrdinalIgnoreCase)) {
    Fail "MSI Start menu shortcut targets '$msiShortcutTarget', expected '$msiExecutable'."
  }
  $capturedMsiRegistryLocations = @($msiRegistrations | ForEach-Object { [string]$_.Location })
  $capturedMsiShortcutPaths = @($msiStartMenuShortcuts | ForEach-Object { [string]$_.Path })
  Invoke-AppAliveSmoke $msiExecutable 'MSI'
  Invoke-MsiProcess $msiExec @('/x', $msiMetadata.ProductCode.ToString('B'), '/qn', '/norestart', '/l*v', $msiUninstallLog) 'MSI silent uninstallation' $msiUninstallLog
  Wait-PathAbsent $msiExecutable 'MSI application executable'
  Wait-PathAbsent $msiInstallRoot 'MSI installation root'
  Wait-MsiComponentAbsent $msiMetadata.ProductCode $msiMetadata.ComponentId
  Wait-MsiProductUnregistered $msiMetadata.ProductCode
  Wait-CapturedArtifactsAbsent $capturedMsiRegistryLocations $capturedMsiShortcutPaths
  Wait-RegistrationArtifactsAbsent $expectedExecutableName $msiInstallRoot $msiMetadata.ProductCode
  $msiInstalled = $false

  if ($OutputDirectory) {
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    $resolvedOutput = (Resolve-Path -LiteralPath $OutputDirectory).ProviderPath
    $normalizedNsis = Join-Path $resolvedOutput "Aseprite-Installer-$StableSuffix-setup.exe"
    $normalizedMsi = Join-Path $resolvedOutput "Aseprite-Installer-$StableSuffix.msi"
    Copy-Item -LiteralPath $nsisPackage -Destination $normalizedNsis
    Copy-Item -LiteralPath $msiPackage -Destination $normalizedMsi

    $nsisSignature = Get-AuthenticodeSignature -LiteralPath $normalizedNsis
    $msiSignature = Get-AuthenticodeSignature -LiteralPath $normalizedMsi
    if ($nsisSignature.Status -ne 'NotSigned' -or $msiSignature.Status -ne 'NotSigned') {
      Fail "release policy requires unsigned Windows packages; signature states are NSIS=$($nsisSignature.Status), MSI=$($msiSignature.Status)."
    }
  }

  Write-Host 'Windows package smoke passed: NSIS and MSI were inspected, installed, launched, and uninstalled.'
} finally {
  if ($msiInstalled) {
    try {
      Invoke-BoundedProcess $msiExec @('/x', $msiMetadata.ProductCode.ToString('B'), '/qn', '/norestart') 'MSI cleanup uninstallation' 120 @(0, 1605)
    } catch {
      Write-Warning $_.Exception.Message
    }
  }
  if ($nsisInstalled -and -not $nsisUninstaller -and (Test-Path -LiteralPath $nsisInstallRoot -PathType Container)) {
    try {
      $cleanupRegistrations = @(Get-RelevantUninstallEntries $expectedExecutableName $nsisInstallRoot | Where-Object {
        $_.Hive -eq 'CurrentUser'
      })
      if ($cleanupRegistrations.Count -eq 1) {
        $candidateUninstaller = Get-RegisteredExecutablePath $cleanupRegistrations[0].UninstallString 'NSIS cleanup HKCU UninstallString'
        if ((Test-PathWithinRoot $candidateUninstaller $nsisInstallRoot) -and
            [IO.Path]::GetFileName($candidateUninstaller) -like 'uninstall*.exe') {
          $nsisUninstaller = $candidateUninstaller
        }
      }
    } catch {
      Write-Warning $_.Exception.Message
    }
  }
  if ($nsisInstalled -and $nsisUninstaller -and (Test-Path -LiteralPath $nsisUninstaller -PathType Leaf)) {
    try {
      Invoke-BoundedProcess $nsisUninstaller @('/S') 'NSIS cleanup uninstallation' 120
    } catch {
      Write-Warning $_.Exception.Message
    }
  }
  if (Test-Path -LiteralPath $nsisInstallRoot -PathType Container) {
    Remove-Item -LiteralPath $nsisInstallRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
  if (Test-Path -LiteralPath $workRoot -PathType Container) {
    Remove-Item -LiteralPath $workRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
