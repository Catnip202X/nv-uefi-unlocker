# RTX 4060 Ti 8 GB (AD106, `10de:2803`) — Windows tensor-rate unlock on ASUS ROG MAXIMUS Z790 APEX

Status: **verified**, 2026-10-27. To our knowledge the first public Windows unlock of an RTX 4060 Ti **8 GB**, and the first on an
ASUS Z790 board. Built on `nv-uefi-unlocker` v2.1.0 plus the loader fixes below, without which this board
resets right after a successful unlock.

## Result

Matched pair: same card, driver and MSI Afterburner profile (power limit 135 % = 216 W, the VBIOS
maximum, plus a custom V/F curve); stock = boot without the stick, unlocked = boot through the stick.

| TFLOPS (cuBLAS, FP32 accumulate) | Stock | Unlocked | Gain |
|---|---:|---:|---:|
| FP16 8192³ | 48.06 | **81.94** | **+70 %** |
| BF16 8192³ | 47.78 | **83.53** | **+75 %** |
| TF32 4096³ | 23.09 | 38.73 | +68 % |
| FP32 4096³ (control) | 12.72 | 12.73 | 0 % |
| FP16 throughput per SM-GHz | ~17.1 | ~34 | **2.0×** |

At the default 160 W (no Afterburner): stock FP16/BF16 46.84 / 47.26 → unlocked 78.64 / 80.25 (+68 % / +70 %).

- Per-clock tensor throughput doubles exactly; the absolute gain is bounded by board power (the unlocked
  card sits on its power limit at both 160 W and 216 W, ~180 W sustained draw).
- **10-minute sustained soak** (FP16/BF16 8192³ alternating, result check vs FP64 every 30 s): PASS,
  mean 79.5 TFLOPS (min 78.2, max 80.8), 2.37–2.43 GHz, ~182 W, max 80 °C, no NVIDIA driver (nvlddmkm)
  or WHEA events. `evidence/ad106-asus-z790/bench/soak-unlocked-216w.json`.
- Numerical check: FP16/BF16 GEMMs vs an FP64 reference, max relative error 3e-4 / 2e-3 in both states
  (normal rounding; the unlock changes issue rate, not results).
- Stock at 216 W is within 2 % of stock at 160 W, so the gain is the unlock, not the power limit.
- Volatile by design: a Windows restart or power-off returns the card to stock (measured); run the stick
  each boot.

Method: `bench/bench_gemm.py` — `torch.mm` on contiguous `randn` inputs, reduced-precision reduction disabled,
5 warm-up + 20 timed iterations, median, two passes; clock/power sampled under load. PyTorch 2.11.0+cu128,
NVIDIA driver 581.42, Windows 10 Pro 19045. Raw JSON for every run: `evidence/ad106-asus-z790/bench/`.

## Hardware

| | |
|---|---|
| GPU | RTX 4060 Ti 8 GB, AD106 rev A1, `10de:2803`, subsystem `1043:88fb`, PCIe 4.0 x8 |
| VBIOS | 95.06.26.00.c0, the ASUS ROG Strix 4060 Ti image (cross-flashed onto this ASUS 4060 Ti 8 GB); power limit 160 W default / 216 W max |
| Board | ASUS ROG MAXIMUS Z790 APEX (AMI UEFI), Intel Core i9-14900K |
| Firmware state | Secure Boot off, Fast Startup off, UEFI boot from a FAT32 USB partition |

## Transaction evidence (`evidence/ad106-asus-z790/unlock3-final-build.log`, the branch head)

```
BRIDGE-BME: (0,0,1,0) command 0x0003 -> 0x0007
FWSEC: WPR2 verified
BOOTER forensic: MAILBOX0=0x67b00001                      <- payload marker
POST-BOOT: FEAT_PLM=0xffffffff
POST-BOOT: WPRCFG=0xfffff0ff FBPA=0xffffff0f WPR=0xffffffff
SELECTOR RESULT: SS0=0x88888888 SS1=0x8
POST-FLR: verified (selectors survive AON)
RECONNECT: pathless GOP survived the detach - vendor GOP only (non-recursive)
RECONNECT: ConnectController ok (recursive=false)
HANDOFF-OUT: ss0=0x88888888 ss1=0x8 readout1=0x00000000 feat_plm=0xffffffff
CHAIN: LoadImage (FromDevicePath)  -> Windows Boot Manager
```

Cold-state notes for this card: SS0 differs every boot (`0x2?026154`) and SS1 does too (`0x2` and `0x6`
observed); neither is a usable stock/unlock discriminator. FBPA PLM is already `0xffffff0f` cold.

## Why stock v2.1.0 resets this board, and the fix

The AMI firmware on this board layers a **pathless GOP splitter child** (no device path, owned by a
firmware console driver) on top of the NVIDIA GOP. The loader's detach removes the NVIDIA GOP, but the
splitter child survives. Afterwards, two **recursive** `ConnectController` calls re-bind the console
stack over that stale child. The platform resets inside the call: the per-line log ends there, and
removing the recursion removes the reset.

1. `tx_detach::reconnect_all` (right after the post-FLR verdict) — the first diagnostic log ends exactly there.
2. `chainload::chain_load_bootmgfw`'s `map -r`-style pass, which recursively connects every PciIo handle,
   the GPU included.

Fixes (branch `asus-z790-gop-reconnect`):

| Commit | Change |
|---|---|
| `c120ad7`, `c3abeec` | reconnect recursively as before, but **vendor GOP only** when a pathless GOP survived the detach; `\no-reconnect.txt` skips the reconnect |
| `5935fda` | skip NVIDIA display functions in the pre-chain reconnect pass |
| `7d4bcad` | flush `loader.log` per line, file before console (a reset no longer leaves a 0-byte log); disable the UEFI watchdog; log the display topology around the detach |

With these, replay and unlock both complete and Windows boots normally. On boards with a single GOP (the
verified H310 desktop) the reconnect stays recursive exactly as before; the pre-chain pass no longer
touches the GPU, which `reconnect_all` has already re-bound.

Side observation: `gop_belongs_to_target` never matches here because the NVIDIA GOP child path carries an
`AcpiAdr` node and the prefix compare includes the PCI path's END node. It is harmless on this board (the
PCI-level recursive disconnect removes the vendor GOP) but worth fixing upstream.

## Reproduce

1. `make-usbroot.ps1 -Family ad102` (v2.1.0 release) — regenerates every firmware asset from pinned public
   inputs; all hashes match the published table (`v67-ad102-four-plm.bin` `44c36b2d…dc8cb`).
2. Build the loader from this branch: `cargo efi-tx` → `\EFI\BOOT\BOOTX64.EFI` on a FAT32 partition.
3. Replay first (no `v67-*.bin` on the stick): expect `TX: execution verified` and a normal Windows boot.
4. Unlock: add `v67-ad102-four-plm.bin`, cold boot, pick the stick from the boot menu, press Space.
5. The unlock is volatile: repeat step 4 after every power-off **and every restart** (measured on this board).

Prerequisites: Secure Boot off, Fast Startup off, Windows on GPT/UEFI.

## Credits

`nv-uefi-unlocker` (physicsdolphin) and its bridge bus-master fix (8bit2qubit, PR #5); `gpu-unlock-ada`
(JustAResearcher) and the four-write Ada payload lineage; TOTORONG's AD103 Windows UEFI port; Zagitta's
original `gpu-unlock`.
