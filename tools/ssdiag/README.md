# ssdiag v0.2 — in-OS NVIDIA register diagnosis + separate-lock re-apply (Windows)

Reads the NVIDIA registers the EFI unlock stage touches — from inside the
running OS — and, where the EFI chain left the FEAT PLM gate open (Ampere/Ada
AON behavior), re-applies the SS override values **without a reboot**
("separate-lock" method, issue #6).

## What it dumps

- multi-GPU: every NVIDIA Display-class function is dumped (companion
  audio/USB-C functions excluded), `--select` narrows.
- core/debug block: PMC_BOOT_0, PSTRAPS (decoded BAR1/BAR0/BAR3 sizes),
  FB_SIZE_MIB, GFW PLM/scratch, BSI_SCRATCH15, VGA_WORKSPACE, WPR2 lo/hi,
  MMU PLM + lock window, GSP/SEC2 CPUCTL + mailboxes + EXCI, fuse
  rows — with `badf` bucketing (PRIV_VIOLATION / UNDECODED / FLOORSWEPT).
- FEAT clusters of BOTH families on every chip: Turing gains READOUT
  (0x409668, per nlzy); plus the Ada four-write targets, the AD10x
  64G-BAR1 line (CYA/CFG/XVE/BAR1 cap mirror), the cmp170hx 26-register
  PLM survey, and the Ampere/Ada FEAT sweep — the union of the register
  surfaces the reference unlockers touch.
- FEAT cluster of BOTH families (Turing 0x4096xx + Ampere/Ada 0x823xxx) on
  every chip, so nothing depends on getting the family route right.
- Verdict: SS override values live = unlocked; gate open + stock values =
  `--reapply` applies (repair path after devinit re-derivation, driver init,
  sleep, or a rail-up Thunderbolt re-enumeration); gate closed (Turing relock
  chain) = read-only by design, re-run the USB unlock.

## Driver: embedded, strict remove-when-done

`pmxdrv.sys` (Intel-signed, hash-pinned against the nvoc tree at build time
and re-verified at startup, fail-closed) is **bundled in the exe**. On run:

1. If a PMXDRV device is already up (e.g. deployed by nvoc), it is borrowed —
   no service changes.
2. Otherwise the driver is extracted to `%TEMP%\ssdiag-pmxdrv.sys`, loaded
   under **its own service name `SSDIAG_PMXDRV`** (kernel, demand-start), and
   on exit — every exit path — the service is stopped, deleted and the temp
   file removed. Nothing is left in the user's service list. Cleanup never
   touches services owned by other tools (the name is ours alone).

Elevation is required for the deploy path (`OpenSCManager`); borrowing an
already-running driver works unelevated only if the device ACL allows it.

## Usage

```text
ssdiag                              # dump every NVIDIA display GPU (auto-discovers BAR0 per card)
ssdiag --reapply                    # + SS0=0x88888888 SS1=0x8 per card where the gate is open
ssdiag --select 1F07                # narrow to functions whose DeviceID matches
```

`--bar0` was removed in v0.3: physical assignments change between UEFI and
Windows (a stale base once hard-hung a machine), so the tool discovers each
GPU's ranges from the OS itself and refuses anything it did not discover.

## Build

```text
cargo build --release        # any Windows MSVC host; no cross toolchain needed
```

## Caveats

- Writes are limited to SS0/SS1 behind an open gate and verified by
  read-back; the tool never writes the PLM register (door-opening stays the
  EFI stage's job).
- BYOVD caveat applies (cmp50hx precedent): some anti-cheats frown on kernel
  driver use — this one lives for the duration of the probe only, but a
  simultaneous anti-cheat session may still complain.
- The transport protocol is copied byte-for-byte from nvoc
  `core/src/kmd/pmxdrv.rs` (reverse-engineered + live-verified there); the
  code table was once mis-derived by hand — do not re-derive it.
