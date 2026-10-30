#Requires -RunAsAdministrator
<#
.SYNOPSIS
Install the unlocker into a dedicated FAT32 partition and add a firmware boot entry,
so the transaction runs at every boot without a USB stick.

.DESCRIPTION
This is the route documented in docs/community-reports.md from issue #33: a small
FAT32 partition whose ROOT holds BOOTX64.EFI and the assets (the loader resolves
every asset path against the root of the volume it booted from, so they cannot live
in a subdirectory), plus one firmware boot entry that the firmware runs before
Windows.

What is written, exactly:
  * <partition>:\BOOTX64.EFI, <partition>:\booter_load_*.bin, and the rest of the
    assets, at the ROOT of the partition you name;
  * one entry in the firmware boot order (NVRAM) and its object in the BCD store;
  * nothing on the ESP beyond the BCD store that bcdedit itself edits, and no file
    of yours anywhere else.

The loader then chain-loads bootmgfw.efi itself, so one boot gives the unlock and
Windows. Nothing is flashed and nothing persists on the GPU.

.PARAMETER Target
Drive letter or volume path of the FAT32 partition to install into. It must be
FAT32 and it must not be the ESP; this script does not create it.

.PARAMETER Source
Output directory of make-usbroot.ps1 (-Out).

.PARAMETER First
Put the new entry first in the firmware boot order. Without it the entry exists but
the firmware may never reach it. On MSI boards the order written by Windows is
ignored anyway - set Boot Option #1 in the BIOS (issue #33).

.PARAMETER AllowArmed
Copy i_know_what_i_am_doing.txt as well, which makes the loader run unattended after
a 5 s countdown. Off by default: an unattended unattended transaction on every boot
is the failure mode this project's confirmation gate exists to prevent.

.PARAMETER Uninstall
Remove the boot entry and the files this script installed, using the manifest it
wrote on first install.

.EXAMPLE
.\install-partition.ps1 -Target D: -Source .\usbroot -First

.EXAMPLE
.\install-partition.ps1 -Target D: -Uninstall
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Target,
    [string]$Source,
    [string]$Description = "NVU unlock",
    [switch]$First,
    [switch]$AllowArmed,
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$LoaderName  = "BOOTX64.EFI"
$ManifestName = "nvu-install-manifest.txt"

function Die($m) { Write-Host "[fail] $m" -ForegroundColor Red; exit 1 }
function Say($m) { Write-Host "[inst] $m" }

# --- resolve and vet the target volume ------------------------------------
$drive = $Target.TrimEnd("\").TrimEnd(":")
if ($drive -notmatch "^[A-Za-z]$") { Die "-Target must be a drive letter (e.g. D:), got '$Target'" }
$root = "${drive}:\"
if (-not (Test-Path $root)) { Die "$root does not exist" }
$vol = Get-Volume -DriveLetter $drive -ErrorAction Stop
if ($vol.FileSystemType -ne "FAT32") {
    Die "$root is $($vol.FileSystemType); the firmware can only read FAT32 here (convert with format, not convert.exe)"
}
if ($vol.DriveType -ne "Fixed") {
    Die "$root is DriveType '$($vol.DriveType)'; this route is for an internal partition. A removable volume is what make-usbroot.ps1 already does."
}

# The ESP is what we are avoiding: refuse to install into it.
try {
    $esp = Get-Partition -DriveLetter $drive -ErrorAction Stop
    if ($esp.GptType -eq "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}") {
        Die "$root is an EFI System Partition; this script installs into a separate partition on purpose (the ESP route is #63's rejected alternative)"
    }
} catch { }

$manifestPath = Join-Path $root $ManifestName

function Read-Manifest {
    if (-not (Test-Path $manifestPath)) { return $null }
    $m = @{ guid = $null; files = @() }
    foreach ($line in Get-Content $manifestPath) {
        if ($line -match "^guid=(.+)$") { $m.guid = $Matches[1] }
        elseif ($line -match "^([0-9a-fA-F]{64})\s\s(.+)$") { $m.files += $Matches[2] }
    }
    return $m
}

# --- uninstall ------------------------------------------------------------
if ($Uninstall) {
    $m = Read-Manifest
    $any = $false
    if ($m -and $m.guid) {
        bcdedit /delete $m.guid /f | Out-Null
        if ($LASTEXITCODE -eq 0) { Say "boot entry $($m.guid) deleted"; $any = $true }
        else { Write-Host "[warn] bcdedit could not delete $($m.guid) (already gone?)" -ForegroundColor Yellow }
    }
    if ($m -and $m.files.Count -gt 0) {
        foreach ($f in $m.files) {
            $p = Join-Path $root $f
            if (Test-Path $p) { Remove-Item -Force $p; Say "removed $f"; $any = $true }
        }
    }
    if (Test-Path $manifestPath) { Remove-Item -Force $manifestPath; $any = $true }
    if (-not $any) { Say "nothing to remove: no $ManifestName on $root" }
    else { Say "done - check the firmware boot order still lists Windows first" }
    return
}

# --- install --------------------------------------------------------------
if (-not $Source) { Die "-Source is required (run make-usbroot.ps1 first)" }
if (-not (Test-Path $Source)) { Die "source directory not found: $Source" }
$srcLoader = Join-Path $Source "BOOTX64.EFI"
if (-not (Test-Path $srcLoader)) { Die "no BOOTX64.EFI in $Source - run make-usbroot.ps1 first" }

$files = Get-ChildItem -Path $Source -File | Where-Object {
    $_.Name -ne "i_know_what_i_am_doing.txt" -or $AllowArmed
}
if (-not $AllowArmed -and (Test-Path (Join-Path $Source "i_know_what_i_am_doing.txt"))) {
    Write-Host "[warn] i_know_what_i_am_doing.txt not copied (armed mode). Pass -AllowArmed if you really want an unattended transaction on every boot." -ForegroundColor Yellow
}
$need = ($files | Measure-Object -Property Length -Sum).Sum
Say ("source {0:N1} MiB, target free {1:N1} MiB of {2:N1} MiB" -f ($need/1MB), ($vol.SizeRemaining/1MB), ($vol.Size/1MB))
if ($need -gt ($vol.SizeRemaining - 16MB)) {
    Die ("not enough space: need {0:N1} MiB with 16 MiB headroom, free {1:N1} MiB" -f ($need/1MB), ($vol.SizeRemaining/1MB))
}

$manifest = @()
foreach ($f in $files) {
    $dst = Join-Path $root $f.Name
    Copy-Item -Force $f.FullName $dst
    $a = (Get-FileHash -Algorithm SHA256 $f.FullName).Hash
    $b = (Get-FileHash -Algorithm SHA256 $dst).Hash
    if ($a -ne $b) { Die "copy verify failed for $($f.Name) - hash mismatch after copy" }
    $manifest += "$a  $($f.Name)"
}
Say ("{0} file(s) verified and installed at {1}" -f $manifest.Count, $root)

# --- firmware boot entry --------------------------------------------------
# A firmware entry is a copy of {bootmgr}; its identifier comes back on stdout.
$old = Read-Manifest
$guid = $null
# Write the manifest with the file list first: if anything below fails after the entry
# exists, -Uninstall still has the record it needs to remove it.
@("guid=") + $manifest | Set-Content -Encoding ascii $manifestPath
$out = bcdedit /copy "{bootmgr}" /d $Description
if ($LASTEXITCODE -ne 0) { Die "bcdedit /copy {bootmgr} failed: $($out -join ' ')" }
if (($out -join " ") -match "(\{[0-9a-fA-F-]{36}\})") { $guid = $Matches[1] }
if (-not $guid) { Die "bcdedit /copy did not return an entry id: $($out -join ' ')" }
@("guid=$guid") + $manifest | Set-Content -Encoding ascii $manifestPath
Say "boot entry $guid created (recorded in $ManifestName)"

bcdedit /set $guid device "partition=${drive}:"  | Out-Null; if ($LASTEXITCODE -ne 0) { Die "bcdedit /set device failed" }
bcdedit /set $guid path "$LoaderName"        | Out-Null; if ($LASTEXITCODE -ne 0) { Die "bcdedit /set path failed" }
if ($First) { bcdedit /set "{fwbootmgr}" displayorder $guid /addfirst | Out-Null; if ($LASTEXITCODE -ne 0) { Die "bcdedit /set {fwbootmgr} displayorder failed" } }

# Verify by value, not by label: read the entry back and compare fields.
$rb = bcdedit /enum $guid
if ($LASTEXITCODE -ne 0) { Die "bcdedit /enum $guid failed after install" }
$text = ($rb -join "`n")
$wantDevice = "partition=${drive}:"
if ($text -notmatch [regex]::Escape($wantDevice) -or $text -notmatch [regex]::Escape($LoaderName)) {
    # Record what was actually written before bailing out, so -Uninstall can still find it.
    @("guid=$guid") + $manifest | Set-Content -Encoding ascii $manifestPath
    Die "read-back does not show $wantDevice / $LoaderName - entry $guid left behind; run -Uninstall"
}

# The entry must never be the default: that turns a refused transaction into a machine
# with no Windows path, and -Uninstall needs a booted Windows.
$cur = (bcdedit /enum "{bootmgr}") -join "`n"
if ($cur -match "default\s+(\{[0-9a-fA-F-]{36}\})" -and $Matches[1] -eq $guid) {
    bcdedit /deletevalue "{bootmgr}" default | Out-Null
    Die "the new entry ended up as the default and the default has been cleared - set Windows as default with bcdedit /set {bootmgr} default {current}, then run -Uninstall"
}

@("guid=$guid") + $manifest | Set-Content -Encoding ascii $manifestPath
Say "installed. Boot menu entry: '$Description' (never set it as the Windows default)"
Say "remove with: .\install-partition.ps1 -Target $($drive): -Uninstall"
if ($First) { Say "if this board ignores the order written here (MSI B550 does), set Boot Option #1 in the BIOS" }
