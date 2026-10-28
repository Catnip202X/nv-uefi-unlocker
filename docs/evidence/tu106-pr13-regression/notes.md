# TU106 (RTX 2070, `10de:1f07`) — merge-PR13 pre-merge regression run, 2026-10-28

Status: **unlock closed, Windows handoff normal** — PR #13's reconnect/chainload
changes verified against an existing-verified machine before merging.

## Build under test

- Branch `merge-pr13` = main(`54d4c53`) + PR #13 (`asus-z790-gop-reconnect`,
  head `77286fb`), local merge `a1646dc`, `cargo efi-tx`.
- EFI SHA-256 `28c3d5f63c2de41200c2b4dcce5e44a685618175986099d8c06a68725d879dc7`,
  TX banner verified by `check_efi.py`.
- Stick: v2.1.4 tu10x layout, `v67-tu10x-four-plm.bin` present (unlock mode).

## Machine

| | |
|---|---|
| GPU | RTX 2070 (TU106, `10de:1f07`) @ 01:00.0, hybrid with Intel UHD 630 (`8086:3e9b`) |
| Board | consumer Z390-class (8086:3e20 host bridge), AMI firmware |
| Entry | cold boot, stick from boot menu, Space confirm |

## Verdict lines

```
TX: execution verified                     <- FEAT_PLM=0xffffffff after booter
SELECTOR RESULT: SS0=0x88888888 SS1=0x8    <- written and read back in the sweep
                                             (0x409650/0x409664/0x40966c)
TU10X PLM relock: plm=0xffffffef SS0=0x88888888 SS1=0x00000008 unlock_persist=true
TX-tu10x: FLR skipped (Turing FEAT PLM does not survive FLR)
HANDOFF-OUT: tu10x_unlock=true
RECONNECT: ConnectController ok (recursive=true)
CHAIN: reconnect skipped on NVIDIA display Handle(0x3a1cf218)
```

PR #13 regression verdict: this machine has **no pathless GOP survivor**, so
`stale_pathless_gop()` kept the reconnect **recursive** — identical to every
previous verified run. The pre-chain pass skipped the NVIDIA display function
as designed. Windows took over normally.

## Finding: third instance of the GOP ownership misjudgment

`DETACH: GOP handle Handle(0x3a0d5f18) is not on the target GPU (multi-GPU)`
fired for the machine's **only** GOP handle — which triple-evidence shows was
the dGPU's own vendor GOP: present before the detach (36 B path), gone after
the dGPU PCI recursive disconnect (`GOP-TOPOLOGY: no GOP handles`), back after
the reconnect (36 B, new handle). The iGPU (`8086:3e9b`) produced no GOP
handle this boot (no console on it), so "1 GOP handles" and "misjudged" do not
contradict.

Mechanism: `gop_belongs_to_target`'s raw byte-prefix compare includes the PCI
path's END node. The NVIDIA GOP child path replaces that END with an
`AcpiAdr(8 B) + END(4 B)` tail — 28 B parent vs 36 B child, so the compare
fails at the parent's END bytes. Same mechanism as the issue #14 flip board
and the ASUS Z790 side observation; this machine is the third instance.

Benign here (false "no" is the safe direction; stage 3 tore the stack anyway),
but the fix is in this branch: `path_belongs_to` (node-aligned compare,
`sim/src/devpath.rs` shape tests).

## Timing observations

- The stage-3 recursive `DisconnectController` (right after the
  "left untouched" line) stalled noticeably: this boot's whole boot console
  stack (ConSplitter + GraphicsConsole, `attr=0x10` opener, `count=317`
  ByHandle user) hung off that GOP, and tearing an active console has
  inherent driver-stop timeouts. Pre-existing behavior, not PR #13.
- PR #13's flush-per-line logging added visible seconds (~415 flushed lines
  on USB FAT). Fixed in this branch: flush every 16th line + stage/verdict
  marker lines (`loader/src/logging.rs`).
