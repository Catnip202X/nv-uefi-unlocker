//! ssdiag - in-OS NVIDIA FEAT-cluster (PLM / SS0 / SS1) reader / re-applier.
//!
//! Reads the same registers the EFI unlock stage touches, from inside the
//! running OS, and - where the EFI chain left the PLM gate open (Ampere/Ada
//! AON behavior, issue #6) - re-applies the SS override values without a
//! reboot ("separate-lock" method: devinit may re-derive rates from fuses,
//! but an open gate lets the host write them back).
//!
//! Transport: the Intel-signed `pmxdrv.sys` (device `\\.\PMXDRV`), the same
//! fully-reversed protocol as nvoc `core/src/kmd/pmxdrv.rs` (map/unmap
//! physical memory into this process, PAGE_READWRITE). The driver is NOT
//! shipped here; install it from the nvoc tree:
//!   sc create PMXDRV type= kernel start= demand binPath=<path>\pmxdrv.sys
//!   sc start PMXDRV
//!
//! BAR0 physical base must be given (--bar0); from PowerShell:
//!   $d = Get-PnpDevice -Class Display | ? InstanceId -match 'VEN_10DE'
//!   Get-CimInstance Win32_PnPAllocatedResource | ? Dependent -match '<DEVID>'
//!   # then the matching Win32_DeviceMemory.StartingAddress
//!
//! Usage:
//!   ssdiag --bar0 0x50000000            # read PMC/PLM/SS0/SS1 (+READOUT)
//!   ssdiag --bar0 0x50000000 --reapply  # SS0=0x88888888 SS1=0x8 when open
//!
//! Read-back of the written values is the only success criterion (const. 1.1).

#![windows_subsystem = "console"]

use std::ffi::CString;
use std::ptr;

// ---- Win32 prototypes (kernel32), no external crates ----
#[allow(non_snake_case)]
mod win {
    use std::ffi::c_void;
    pub type Handle = *mut c_void;
    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            sa: *const c_void,
            disposition: u32,
            flags: u32,
            template: *const c_void,
        ) -> Handle;
        pub fn DeviceIoControl(
            h: Handle,
            ioctl: u32,
            inbuf: *const c_void,
            inlen: u32,
            outbuf: *mut c_void,
            outlen: u32,
            returned: *mut u32,
            overlapped: *const c_void,
        ) -> i32;
        pub fn CloseHandle(h: Handle) -> i32;
        pub fn GetLastError() -> u32;
    }
}

const GENERIC_RW: u32 = 0xC000_0000;
const OPEN_EXISTING: u32 = 3;

// ---- pmxdrv protocol (byte layout copied verbatim from nvoc
// core/src/kmd/pmxdrv.rs - the code table was reverse-engineered and
// live-verified there; do NOT re-derive it) ----
const DEVICE_PATH: &str = r"\\.\PMXDRV";
const IOCTL_MAP_PHYS: u32 = 0x222A_B8;
const IOCTL_UNMAP_PHYS: u32 = 0x222A_BC;
const INPUT_LEN: usize = 16; // [u64 user-mode request ptr][u32 aux][u32 pad]
const REQ_LEN: u32 = 24; // request tag = struct length
const MAP_FAIL_MARK: u64 = 0xBEEF;

struct PmxDrv {
    handle: win::Handle,
}

impl PmxDrv {
    fn open() -> Result<Self, String> {
        let wide: Vec<u16> = DEVICE_PATH.encode_utf16().chain([0]).collect();
        let h = unsafe {
            win::CreateFileW(
                wide.as_ptr(),
                GENERIC_RW,
                3, // FILE_SHARE_READ | FILE_SHARE_WRITE
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null(),
            )
        };
        if h as isize == -1 {
            let err = unsafe { win::GetLastError() };
            return Err(format!(
                "cannot open {DEVICE_PATH} (win32 error {err}) - driver not installed? \
                 admin: sc create PMXDRV type= kernel start= demand binPath=<path>pmxdrv.sys && sc start PMXDRV"
            ));
        }
        Ok(Self { handle: h })
    }

    fn forward(&self, ioctl: u32, request: &mut [u8]) -> Result<(), u32> {
        let mut input = [0u8; INPUT_LEN];
        input[..8].copy_from_slice(&(request.as_ptr() as u64).to_le_bytes());
        let mut returned: u32 = 0;
        let ok = unsafe {
            win::DeviceIoControl(
                self.handle,
                ioctl,
                input.as_ptr().cast(),
                input.len() as u32,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null(),
            )
        };
        if ok == 0 {
            return Err(unsafe { win::GetLastError() });
        }
        Ok(())
    }

    /// Map `pages` pages of physical memory into this process (RW).
    fn map_physical(&self, pa: u64, pages: u32) -> Result<u64, String> {
        let mut request = [0u8; 24];
        request[0..4].copy_from_slice(&REQ_LEN.to_le_bytes());
        request[4..12].copy_from_slice(&pa.to_le_bytes());
        request[12..16].copy_from_slice(&pages.to_le_bytes());
        self.forward(IOCTL_MAP_PHYS, &mut request)
            .map_err(|e| format!("MAP_PHYS ioctl failed (win32 error {e})"))?;
        let va = u64::from_le_bytes(request[16..24].try_into().unwrap());
        if va == 0 || va == MAP_FAIL_MARK {
            return Err("map rejected by driver".into());
        }
        Ok(va)
    }

    fn unmap_physical(&self, va: u64) {
        let mut request = [0u8; 24];
        request[0..4].copy_from_slice(&REQ_LEN.to_le_bytes());
        request[16..24].copy_from_slice(&va.to_le_bytes());
        let _ = self.forward(IOCTL_UNMAP_PHYS, &mut request);
    }
}

impl Drop for PmxDrv {
    fn drop(&mut self) {
        unsafe {
            win::CloseHandle(self.handle);
        }
    }
}

// ---- FEAT cluster (same tables as tools/diag-unlock.py) ----
const SS0_UNLOCK: u32 = 0x8888_8888;
const SS1_UNLOCK: u32 = 0x8;
const TURING: [( &str, u32); 3] = [("PLM", 0x0040_9650), ("SS0", 0x0040_9664), ("SS1", 0x0040_966C)];
const GAADA: [(&str, u32); 4] = [
    ("PLM", 0x0082_3804),
    ("SS0", 0x0082_381C),
    ("SS1", 0x0082_3820),
    ("READOUT", 0x0082_3818),
];

fn family_of(boot0: u32) -> Option<(&'static str, &'static [(&'static str, u32)])> {
    let die = (boot0 >> 20) & 0xFFF;
    if (0x160..=0x16F).contains(&die) {
        Some(("turing", &TURING))
    } else if (0x170..=0x17F).contains(&die) || (0x220..=0x22F).contains(&die) || die == 0x100 {
        Some(("ampere/ada", &GAADA))
    } else {
        None
    }
}

fn decode_plm(v: u32) -> &'static str {
    match v {
        0xFFFF_FFFF => "OPEN",
        0xFFFF_FF8F => "closed (stock)",
        0xFFFF_FFEF => "closed (relock+drift)",
        _ => "closed (non-standard)",
    }
}

fn decode_ss(v: u32, width: u32) -> String {
    let names = [".", "1/2", "1/4", "1/8"];
    (0..width)
        .map(|i| {
            let nib = (v >> (4 * i)) & 0xF;
            if nib == 0 {
                ".".to_string()
            } else if nib & 0x8 != 0 {
                format!("{}*", names[(nib & 7) as usize].trim_start_matches("1/"))
            } else {
                format!("r{}", nib & 7)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() {
    let mut bar0: Option<u64> = None;
    let mut reapply = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bar0" => {
                let v = args.next().unwrap_or_else(|| {
                    eprintln!("--bar0 needs a value (e.g. 0x50000000)");
                    std::process::exit(2);
                });
                let s = v.trim_start_matches("0x").trim_start_matches("0X");
                bar0 = Some(u64::from_str_radix(s, 16).unwrap_or_else(|_| {
                    eprintln!("bad --bar0 value {v:?}");
                    std::process::exit(2);
                }));
            }
            "--reapply" => reapply = true,
            other => {
                eprintln!("unknown arg {other:?}; usage: ssdiag --bar0 0x50000000 [--reapply]");
                std::process::exit(2);
            }
        }
    }
    let Some(bar0) = bar0 else {
        eprintln!("usage: ssdiag --bar0 <BAR0 physical base, e.g. 0x50000000> [--reapply]");
        eprintln!("BAR0 base: Device Manager -> GPU -> Resources, or the PowerShell snippet in tools/ssdiag/README.md");
        std::process::exit(2);
    };

    let drv = match PmxDrv::open() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    const PAGES: u32 = 4096; // 16 MiB BAR0
    let va = match drv.map_physical(bar0, PAGES) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("map BAR0@{bar0:#x} failed: {e}");
            std::process::exit(1);
        }
    };
    let base = va as *const u8;
    let rd = |off: u32| unsafe { ((base.add(off as usize)) as *const u32).read_volatile() };
    let wr = |off: u32, v: u32| unsafe { ((base.add(off as usize)) as *mut u32).write_volatile(v) };

    let boot0 = rd(0x0);
    let Some((fam, regs)) = family_of(boot0) else {
        eprintln!("PMC_BOOT_0={boot0:#010x} - unsupported chip");
        drv.unmap_physical(va);
        std::process::exit(1);
    };
    let get = |name: &str| regs.iter().find(|(n, _)| *n == name).unwrap().1;
    let plm = rd(get("PLM"));
    let ss0 = rd(get("SS0"));
    let ss1 = rd(get("SS1"));
    println!("PMC_BOOT_0={boot0:#010x} family={fam}");
    println!("FEAT_PLM  = {plm:#010x} ({})", decode_plm(plm));
    println!("FEAT_SS0  = {ss0:#010x} [{}]", decode_ss(ss0, 8));
    println!("FEAT_SS1  = {ss1:#010x} [{}]", decode_ss(ss1, 1));
    if let Some((_, ro_off)) = regs.iter().find(|(n, _)| *n == "READOUT") {
        let ro = rd(*ro_off);
        println!("READOUT_1 = {ro:#010x}");
    }
    let unlocked = ss0 == SS0_UNLOCK && ss1 == SS1_UNLOCK;
    println!(
        "VERDICT: {}",
        if unlocked { "UNLOCKED (override values live)" } else { "not unlocked" }
    );

    let mut rc = 0;
    if reapply {
        if plm != 0xFFFF_FFFF {
            println!("reapply REFUSED: FEAT PLM gate is closed - re-run the USB unlock instead");
            rc = 1;
        } else {
            wr(get("SS0"), SS0_UNLOCK);
            wr(get("SS1"), SS1_UNLOCK);
            let rb0 = rd(get("SS0"));
            let rb1 = rd(get("SS1"));
            let ok = rb0 == SS0_UNLOCK && rb1 == SS1_UNLOCK;
            println!(
                "reapply: SS0={rb0:#010x} SS1={rb1:#010x} -> {}",
                if ok { "OK" } else { "READBACK MISMATCH" }
            );
            rc = if ok { 0 } else { 1 };
        }
    }
    drv.unmap_physical(va);
    std::process::exit(rc);
}

// keep CString import used only if device path needed ASCII fallback later
#[allow(dead_code)]
fn _ascii(s: &str) -> CString {
    CString::new(s).unwrap()
}
