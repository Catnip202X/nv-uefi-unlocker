# Downstream tools (unofficial)

**This directory is not part of nv-uefi-unlocker.** Nothing here is reviewed against
`AGENTS.md`, nothing here is supported, and upstream will not take PRs for it. It is the
author's fork carrying conveniences that the project deliberately does not own.

Reason: the project's safety model is "boot from a stick, and the worse case is unplugging
it". Tools that persist a boot entry change that model, so they live outside the repo whose
constitution they would bend. Ask upstream for a fix to the unlocker; ask here for this.

---

## install-partition.ps1 - run without a USB stick

The route is the one field-tested in issue #33, not an invention: a small FAT32 partition holding the assets at its **root**, plus one firmware boot entry.
It adds no file to the EFI System Partition, but it does add an entry to the BCD store, which lives there. That part is unavoidable: every boot entry is a BCD object.

## Why the root of its own partition

`loader/src/tx.rs` resolves every asset as `\name` against the root of the volume the image
was loaded from (`open_volume()`). There is no image-relative lookup, so:

- a subdirectory (`\EFI\NVU\`) does not work at all - the loader would find no assets and
  would refuse or run in the wrong mode;
- the assets have to sit at the root of whatever volume `BOOTX64.EFI` was loaded from.

That is also what `README.en.md` tells USB users ("the `.bin` files and optional marker files
at the stick root"), so this route is the same convention with a different volume.

## What is written

| where | what |
|---|---|
| `<partition>:\` | `BOOTX64.EFI`, the family's `.bin` assets, optional markers, `nvu-install-manifest.txt` |
| firmware NVRAM | one boot entry, a copy of `{bootmgr}` with `device=<partition>`, `path=BOOTX64.EFI` |
| the ESP's BCD store | the object for that entry - written by `bcdedit` itself; this is the one thing on the ESP that changes, and it is unavoidable |

`loader.log` is written to the root of that partition on every boot, as it is on a stick.

The only way past the ESP refusal is `-Force`, which prints a warning and continues; it exists so a machine whose partition table cannot be read is not a dead end, and it is the sole path by which this script will ever write into an ESP.

## The three prohibitions

1. **Never make the entry the Windows default.** If the transaction refuses or fails, the
   machine would come up with no Windows path, and `-Uninstall` needs a booted Windows. The
   script never writes `{bootmgr} default` at all, which is stronger than checking afterwards;
   it is your job not to run `bcdedit /set {bootmgr} default` afterwards either.
2. **Do not copy `i_know_what_i_am_doing.txt`.** It turns the 5 s confirmation into an
   unattended run on every boot. The script skips it unless `-AllowArmed` is given.
3. **The assets are not the firmware you already trust.** A damaged `.bin` on an internal
   partition is not visible until the next boot; the USB route at least forces you to handle
   the medium.

## Recovery cost, stated plainly

If the entry boots but does not chain-load, the machine stops before Windows. Recovery needs
a Windows installation medium (or the firmware's own boot menu to pick Windows directly, which
most boards offer under F7/F11/F12 at power-on). That is a strictly worse failure mode than
"unplug the stick", and it is why this route is documented as an option, not as the default.

On MSI boards the order written by `bcdedit` to `{fwbootmgr}` is ignored; set Boot Option #1
in the BIOS (issue #33).

## Not verified

The script has not been run here: there is no Windows host and no spare partition to install
into, and rehearsing on someone's disk is not acceptable. What is verified is that each step
is the documented interface (`mountvol` is not used at all, `bcdedit /copy {bootmgr}`, `/set
device`, `/set path`, `{fwbootmgr} displayorder ... /addfirst`), that every write is followed
by a read-back, and that the copies are hash-verified. The community report in #33 is the
hardware evidence for the route itself.
