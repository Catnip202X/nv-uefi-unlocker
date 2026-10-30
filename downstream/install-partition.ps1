#Requires -RunAsAdministrator
<#
.SYNOPSIS
Install the unlocker into a dedicated FAT32 partition and add one firmware boot entry,
so the transaction can run without a USB stick.

.DESCRIPTION
Route documented in docs/community-reports.md from issue #33: a FAT32 partition whose
ROOT holds BOOTX64.EFI and the selected family's assets (loader/src/tx.rs resolves every
asset as "\name" against the root of the volume it booted from - there is no
image-relative lookup, so a subdirectory does not work), plus one firmware boot entry.

What is written:
  * <partition>:\BOOTX64.EFI and the family's expected .bin set, at the ROOT;
  * nvu-install-manifest.txt at the root (file hashes + boot entry id);
  * one boot entry: a copy of {bootmgr} with device=<partition> and path=\BOOTX64.EFI;
  * the BCD store object for that entry, written by bcdedit itself.

Nothing is written to the EFI System Partition by this script apart from what bcdedit
does to the BCD store, which any boot entry requires. Nothing is flashed; the unlock is
volatile and is replayed by the loader on each boot.

.PARAMETER Target
Drive letter of the FAT32 partition to install into (e.g. D:). It must be FAT32, it must
be a fixed disk, and it must not be the ESP. This script does not create the partition.

.PARAMETER Source
Output directory of make-usbroot.ps1 (-Out).

.PARAMETER Family
Which asset set to copy: ad102, tu10x, ... Defaults to the family subdirectory or file
prefix found in -Source. Only that set is copied - the maker output also contains
intermediate files, and the loader pins per-family names.

.PARAMETER First
Add the entry to the front of the firmware boot order. Omitted, the entry exists but is
only reachable from the firmware's own boot menu. On MSI boards the order written here is
ignored; set Boot Option #1 in the BIOS (issue #33).

.PARAMETER AllowArmed
Also copy i_know_what_i_am_doing.txt, which makes the loader run unattended after a 5 s
countdown. Off by default: an unattended transaction on every boot is exactly what the
confirmation gate exists to prevent.

.PARAMETER Uninstall
Delete the boot entry and the files recorded in the manifest.

.EXAMPLE
.\install-partition.ps1 -Target D: -Source .\usbroot -Family ad102

.EXAMPLE
.\install-partition.ps1 -Target D: -Uninstall
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Target,
    [string]$Source,
    [string]$Family,
    [string]$Description = "NVU unlock",
    [switch]$First,
    [switch]$AllowArmed,
    [switch]$Force,
    [switch]$ClearStale,
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$LoaderName   = "BOOTX64.EFI"
$ManifestName = "nvu-install-manifest.txt"

function Die($m) { Write-Host "[fail] $m" -ForegroundColor Red; exit 1 }
function Say($m) { Write-Host "[inst] $m" }
function Warn($m) { Write-Host "[warn] $m" -ForegroundColor Yellow }

$drive = $Target.TrimEnd("\").TrimEnd(":")
if ($drive -notmatch "^[A-Za-z]$") { Die "-Target must be a drive letter such as D:, got '$Target'" }
$root = "${drive}:\"
$wantDevice = "partition=${drive}:"
if (-not (Test-Path $root)) { Die "$root does not exist" }

$vol = Get-Volume -DriveLetter $drive -ErrorAction Stop
if ($vol.FileSystemType -ne "FAT32") {
    Die "$root is $($vol.FileSystemType); the firmware can only read FAT32 here (format the partition, do not convert)"
}
if ($vol.DriveType -ne "Fixed") {
    Die "$root is DriveType '$($vol.DriveType)'. Use make-usbroot.ps1 for removable media."
}
$espByContent = Test-Path -LiteralPath (Join-Path $root "EFI\Microsoft\Boot\bootmgfw.efi")
try {
    $part = Get-Partition -DriveLetter $drive -ErrorAction Stop
    $isEsp = ($part.GptType -eq "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}") -or ($part.MbrType -eq 0xEF)
    if ($espByContent) {
        $isEsp = $true
    }
    if ($isEsp) {
        if (-not $Force) { Die "$root is an EFI System Partition - this route installs into a separate partition on purpose (see issue #73)" }
        Warn "-Force given: installing into the ESP anyway, which #73 rejected"
    }
} catch {
    # Failing open here would defeat the whole point; ask for an explicit override instead.
    if ($espByContent -and -not $Force) { Die "$root carries \EFI\Microsoft\Boot\bootmgfw.efi, so it is the ESP - this route installs into a separate partition (see issue #73)" }
    if (-not $Force) { Die "could not read the partition table for ${drive}: ($($_.Exception.Message)). Pass -Force only if you are certain $root is not the ESP." }
    Warn "partition table unreadable and -Force given - continuing without the ESP check"
}


$manifestPath = Join-Path $root $ManifestName

function Read-Manifest {
    if (-not (Test-Path $manifestPath)) { return $null }
    $m = @{ guid = $null; files = @() }
    foreach ($line in Get-Content $manifestPath) {
        if ($line -match "^guid=(\{[0-9a-fA-F-]{36}\})?$") { $m.guid = if ($Matches[1]) { $Matches[1] } else { $null } }
        elseif ($line -match "^([0-9a-fA-F]{64})\s\s(.+)$") { $m.files += $Matches[2] }
    }
    return $m
}
function Save-Manifest([string]$guid, [string[]]$files) {
    $lines = @("guid=$guid") + ($files | ForEach-Object { $_ })
    Set-Content -Path $manifestPath -Value $lines -Encoding ascii
}

# --- uninstall ------------------------------------------------------------
if ($Uninstall) {
    $m = Read-Manifest
    $any = $false
    if ($m -and $m.guid) {
        bcdedit /delete $m.guid /f | Out-Null
        if ($LASTEXITCODE -eq 0) { Say "boot entry $($m.guid) deleted"; $any = $true }
        else { Warn "bcdedit could not delete $($m.guid) - check bcdedit /enum {fwbootmgr}" }
    }
    if ($m -and $m.files.Count -gt 0) {
        foreach ($f in $m.files) {
            if (Test-Path -LiteralPath (Join-Path $root $f)) {
                Remove-Item -LiteralPath (Join-Path $root $f) -Force
                Say "removed $f"
                $any = $true
            }
        }
    }
    if (Test-Path $manifestPath) { Remove-Item -LiteralPath $manifestPath -Force; $any = $true }
    $fw = (bcdedit /enum "{fwbootmgr}" /v) -join "`n"
    if ($fw -match "displayorder") { Say "firmware boot order after removal:"; ($fw -split "`n" | Select-String -Pattern "\{" ) | ForEach-Object { "       " + $_.Line.Trim() } }
    if (-not $any) { Say "nothing to remove" } else { Say "done - confirm Windows is still the first boot option" }
    return
}

# Secure Boot cannot load an unsigned image; AGENTS.md §1.9 makes checking this mandatory.
try {
    if (Confirm-SecureBootUEFI) {
        Die "Secure Boot is ON and this loader is unsigned - the entry would fail to load. Disable Secure Boot (or enrol the signing certificate) first."
    }
} catch {
    Die "Confirm-SecureBootUEFI failed - this machine is not booting in UEFI mode, so there is no firmware boot entry to create"
}

# --- install --------------------------------------------------------------
if (-not $Source) { Die "-Source is required (run make-usbroot.ps1 first)" }
if (-not (Test-Path $Source)) { Die "source directory not found: $Source" }
if (-not (Test-Path (Join-Path $Source $LoaderName))) { Die "no $LoaderName in $Source - run make-usbroot.ps1 first" }

# The maker emits every family plus intermediates; install only one asset set.
if (-not $Family) {
    $cand = Get-ChildItem -Path $Source -File -Filter "booter_load_*.bin" | Select-Object -First 1
    if (-not $cand) { Die "cannot infer -Family from $Source (no booter_load_*.bin); pass -Family explicitly" }
    if ($cand.Name -match "^booter_load_(.+)\.bin$") { $Family = $Matches[1] } else { Die "cannot parse family from $($cand.Name)" }
    Say "family inferred from $($cand.Name): $Family"
}
$known = @("ad102", "ga102", "tu10x")
if ($known -notcontains $Family) {
    Die "family '$Family' is not one the loader routes ($($known -join ', ')); refusing to install an asset set it will ignore"
}
# Expected per-family set, from README.en.md:76 and the loader's own lookups.
$expected = @(
    $LoaderName,
    "booter_load_$Family.bin",
    "gsp_boot_$Family.bin",
    "gsp_boot_desc_$Family.bin",
    "fwimage_$Family.bin",
    "fwsignature_$Family.bin"
)
# Turing fires the booter through the SEC2 bootloader; without it the install is a no-op.
if ($Family -eq "tu10x") { $expected += "sec2_bl_tu10x.bin" }
$optional = @("v67-$Family-four-plm.bin", "scrubber_ad10x.bin", "tu10x-fb-gb.txt", "tx-select-bdf.txt", "tx-low-staging.txt", "pre-flr-on.txt", "tx-force-continue.txt", "no-reconnect.txt", "tx-gop-disconnect.txt", "tx-require-d0.txt", "tx-meta-in-vram.txt", "tu10x-no-ss.txt", "tu10x-no-reconnect.txt")
$armed = @("i_know_what_i_am_doing.txt")

$missing = @($expected | Where-Object { -not (Test-Path -LiteralPath (Join-Path $Source $_)) })
if ($missing.Count -gt 0) { Die ("source is missing required asset(s) for family {0}: {1}" -f $Family, ($missing -join ", ")) }

$toCopy = @()
foreach ($n in $expected) { $toCopy += $n }
foreach ($n in $optional) { if (Test-Path -LiteralPath (Join-Path $Source $n)) { $toCopy += $n } }
if ($AllowArmed) {
    foreach ($n in $armed) { if (Test-Path -LiteralPath (Join-Path $Source $n)) { $toCopy += $n } }
} elseif (Test-Path -LiteralPath (Join-Path $Source "i_know_what_i_am_doing.txt")) {
    Warn "i_know_what_i_am_doing.txt NOT copied (armed mode). Pass -AllowArmed to install an unattended transaction on every boot."
}

# A marker left by an earlier install changes behaviour even if this run does not copy one.
if (-not $AllowArmed -and (Test-Path -LiteralPath (Join-Path $root "i_know_what_i_am_doing.txt"))) {
    Warn "removing a stale i_know_what_i_am_doing.txt from the target (armed mode not requested)"
    Remove-Item -LiteralPath (Join-Path $root "i_know_what_i_am_doing.txt") -Force
    if (Test-Path -LiteralPath (Join-Path $root "i_know_what_i_am_doing.txt")) {
        Die "could not remove the stale i_know_what_i_am_doing.txt - an unattended transaction would run on the next boot"
    }
}

$need = 0
foreach ($n in $toCopy) { $need += (Get-Item -LiteralPath (Join-Path $Source $n)).Length }
# FAT32 allocates per cluster; assume 32 KiB clusters and round up, plus 16 MiB headroom.
$need += [math]::Ceiling($toCopy.Count / 1) * 0 + ($toCopy.Count * 32KB)
Say ("{0} file(s), {1:N1} MiB incl. cluster slack; target free {2:N1} MiB" -f $toCopy.Count, ($need/1MB), ($vol.SizeRemaining/1MB))
if ($need -gt ($vol.SizeRemaining - 16MB)) {
    Die ("not enough space: need {0:N1} MiB with 16 MiB headroom, free {1:N1} MiB" -f ($need/1MB), ($vol.SizeRemaining/1MB))
}

$old = Read-Manifest
$hashes = @()
$guid = $null
try {
    foreach ($n in $toCopy) {
        $src = Join-Path $Source $n
        $dst = Join-Path $root $n
        Copy-Item -LiteralPath $src -Destination $dst -Force
        $a = (Get-FileHash -Algorithm SHA256 -LiteralPath $src).Hash
        $b = (Get-FileHash -Algorithm SHA256 -LiteralPath $dst).Hash
        if ($a -ne $b) { throw "copy verify failed for $n (hash mismatch after copy)" }
        $hashes += "$a  $n"
    }
    Say ("{0} file(s) verified at {1}" -f $hashes.Count, $root)

    # Markers left by an earlier install change behaviour even when this run copies none.
    $stale = @()
    foreach ($m in (Get-ChildItem -Path $root -File -ErrorAction SilentlyContinue)) {
        if ($m.Name -eq $ManifestName) { continue }
        if ($toCopy -contains $m.Name) { continue }
        if ($m.Extension -in @(".txt", ".bin")) { $stale += $m.Name }
    }
    if ($stale.Count -gt 0) {
        Warn ("file(s) already on the target that this run does not install: " + ($stale -join ", "))
        Warn "they are read by the loader at boot - v67-direct-ss.bin outranks the hash-verified payload, and a foreign *_<family>.bin can be picked up by name"
        if ($ClearStale) {
            foreach ($n in $stale) {
                Remove-Item -LiteralPath (Join-Path $root $n) -Force
                Say "removed stale $n"
            }
        } else {
            Warn "pass -ClearStale to remove them, or delete them by hand before booting this entry"
        }
    }


    # A previous install is replaced, not duplicated.
    if ($old -and $old.guid) {
        bcdedit /delete $old.guid /f | Out-Null
        Say "previous entry $($old.guid) removed"
    }
    # Record the file list before the entry exists, so -Uninstall can always clean up.
    Save-Manifest "" $hashes

    $out = bcdedit /copy "{bootmgr}" /d $Description
    if ($LASTEXITCODE -ne 0) { throw "bcdedit /copy {bootmgr} failed: $($out -join ' ')" }
    if (($out -join " ") -notmatch "(\{[0-9a-fA-F-]{36}\})") { throw "bcdedit /copy returned no entry id: $($out -join ' ')" }
    $guid = $Matches[1]
    Save-Manifest $guid $hashes
    Say "boot entry $guid created"

    # path is a BCD element: it is relative to the device, and it needs the leading backslash.
    bcdedit /set $guid device $wantDevice | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "bcdedit /set device $wantDevice failed" }
    bcdedit /set $guid path "\$LoaderName" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "bcdedit /set path \${LoaderName} failed" }

    if ($First) {
        bcdedit /set "{fwbootmgr}" displayorder $guid /addfirst | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "bcdedit /set {fwbootmgr} displayorder /addfirst failed" }
        # Verify the order rather than trusting it: the GUID must appear before Windows' own entry.
        $fw = (bcdedit /enum "{fwbootmgr}" /v) -join "`n"
        # Report the order as it is. The element is a list of GUIDs; {bootmgr} is not one
        # of them, so only our own position in that list can be checked.
        $lines = $fw -split "`n"
        $order = @()
        $inOrder = $false
        foreach ($l in $lines) {
            if ($l -match "displayorder") {
                $inOrder = $true
                # bcdedit prints the first member on the keyword line itself.
                foreach ($m in [regex]::Matches($l, "\{[0-9a-fA-F-]{36}\}")) { $order += $m.Value }
                continue
            }
            if ($inOrder) {
                $t = $l.Trim()
                if ($t -match "^\{[0-9a-fA-F-]{36}\}$") { $order += $t }
                elseif ($t) { break }
            }
        }
        if ($order.Count -eq 0) {
            Warn "{fwbootmgr} shows no readable displayorder after -First. Set Boot Option #1 in the firmware setup and check it there."
        } else {
            Say ("firmware boot order: " + (($order | ForEach-Object { $_ }) -join " -> "))
            $idx = [array]::IndexOf($order, $guid)
            if ($idx -lt 0) { Warn "the new entry is not in {fwbootmgr} displayorder - the firmware may never run it. Set Boot Option #1 in the setup." }
            elseif ($idx -eq 0) { Say "the new entry is first in the firmware order" }
            else { Warn "the new entry is at position $($idx + 1) of $($order.Count) - the firmware will run the earlier one(s) first. Set Boot Option #1 in the setup." }
        }
    }

    # Read back what the entry actually contains; bcdedit prints device as "partition=D:".
    $rb = (bcdedit /enum $guid) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw "bcdedit /enum $guid failed after install" }
    if ($rb -notmatch [regex]::Escape($wantDevice)) { throw "read-back does not show device=$wantDevice" }
    if ($rb -notmatch [regex]::Escape("\$LoaderName")) { throw "read-back does not show path=\$LoaderName" }
    Say "read-back verified: device=$wantDevice path=\$LoaderName"
}
catch {
    Write-Host "[fail] $($_.Exception.Message)" -ForegroundColor Red
    if ($guid) {
        bcdedit /delete $guid /f | Out-Null
        if ($LASTEXITCODE -eq 0) { Write-Host "[fail] entry $guid removed again" -ForegroundColor Red }
        else { Write-Host "[fail] could NOT remove entry $guid - remove it by hand with: bcdedit /delete $guid /f" -ForegroundColor Red }
    }
    if ($hashes.Count -gt 0) {
        foreach ($h in $hashes) {
            $n = ($h -split "  ", 2)[1]
            if (Test-Path -LiteralPath (Join-Path $root $n)) { Remove-Item -LiteralPath (Join-Path $root $n) -Force -ErrorAction SilentlyContinue }
        }
        Write-Host "[fail] copied files removed again" -ForegroundColor Red
    }
    # Keep the manifest if an entry may still exist: it is the only handle -Uninstall has.
    $leftover = $false
    if ($guid) {
        bcdedit /enum $guid | Out-Null
        if ($LASTEXITCODE -eq 0) { $leftover = $true; Write-Host "[fail] entry $guid STILL EXISTS - run: .\install-partition.ps1 -Target ${drive}: -Uninstall" -ForegroundColor Red }
    }
    if (-not $leftover -and (Test-Path $manifestPath)) { Remove-Item -LiteralPath $manifestPath -Force -ErrorAction SilentlyContinue }
    exit 1
}

Say "installed. Entry: '$Description' - it is never made the Windows default by this script"
if ($First) { Say "on MSI boards the order above is ignored: set Boot Option #1 in the BIOS (issue #33)" }
Say "NEXT STEP (required): reboot, pick '$Description' in the firmware menu, and confirm"
Say "  - the loader shows its banner and the transaction verdict"
Say "  - Windows then starts (the loader chain-loads bootmgfw.efi itself)"
Say "  - $root\loader.log exists and ends with the handoff line"
Say "if Windows does not start, reboot and pick Windows in the firmware menu, then run: .\install-partition.ps1 -Target ${drive}: -Uninstall"
