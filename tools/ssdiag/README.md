# ssdiag — in-OS FEAT-cluster (PLM / SS0 / SS1) reader / re-applier (Windows)

Reads the same NVIDIA registers the EFI unlock stage touches — from inside the
running OS — and, where the EFI chain left the FEAT PLM gate open (Ampere/Ada
AON behavior), re-applies the SS override values **without a reboot**
("separate-lock" method, issue #6).

## Transport: `pmxdrv.sys` (NOT shipped here)

Intel-signed (`Intel(R) Embedded Subsystems and IP Blocks Group`, 2019 build),
reverse-engineered + live-verified in the nvoc tree
(`core/src/kmd/pmxdrv.rs` — byte-level protocol copied from there verbatim).
Not on Microsoft's vulnerable-driver blocklist as of 2026-10 (verified Win11
26200, HVCI off). Copy `pmxdrv.sys` from the nvoc tree, then (elevated):

```text
sc create PMXDRV type= kernel start= demand binPath=C:\path\pmxdrv.sys
sc start PMXDRV
```

## Find BAR0 physical base

```powershell
$gpu = Get-PnpDevice -Class Display | Where-Object InstanceId -match 'VEN_10DE'
Get-CimInstance Win32_PnPAllocatedResource |
  Where-Object { $_.Dependent -eq $gpu.DeviceID -and $_.Antecedent -match 'Memory' }
# follow the Antecedent -> Win32_DeviceMemory -> StartingAddress
```

## Run

```text
ssdiag --bar0 0x50000000            # read PMC / FEAT_PLM / SS0 / SS1 (+READOUT on Ampere/Ada)
ssdiag --bar0 0x50000000 --reapply  # SS0=0x88888888 SS1=0x8 while the gate is open
```

Verdict semantics:

- `FEAT_SS0=0x88888888 SS1=0x8` → override values live = unlocked, regardless
  of gate state.
- `PLM OPEN` + stock values → the door is open, `--reapply` applies (this is
  the repair path for "devinit re-derived the rates" after driver init /
  sleep / Thunderbolt re-enumeration).
- `PLM closed` (Turing relock chain) → host writes are dropped by hardware;
  re-run the USB unlock instead. Reads still work (diagnosis on Turing is
  read-only by design).

## Caveats

- Run elevated (kernel driver, service start).
- The tool never writes the PLM register itself: door-opening stays the EFI
  stage's job. `--reapply` only writes SS0/SS1 and verifies by read-back.
- BYOVD caveat applies (cmp50hx precedent): some anti-cheats frown on kernel
  driver use; the driver is demand-start and can be stopped/removed after use
  (`sc stop PMXDRV && sc delete PMXDRV`).
