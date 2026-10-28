//! ssdiag v0.2 - in-OS NVIDIA register diagnosis + separate-lock re-apply.
//!
//! Reads the registers the EFI unlock stage touches, from inside the running
//! OS, and - where the EFI chain left the FEAT PLM gate open (Ampere/Ada AON
//! behavior, issue #6) - re-applies the SS override values without a reboot.
//!
//! Driver lifecycle (strict, cmp50hx "remove-when-done" model): the
//! Intel-signed pmxdrv.sys is EMBEDDED, extracted to %TEMP% on demand,
//! loaded under its own service name (SSDIAG_PMXDRV), and STOPPED + DELETED
//! on exit - nothing stays in the user's service list. If a PMXDRV-class
//! driver is already running (e.g. deployed by nvoc), it is borrowed and
//! left untouched. Requires elevation for the deploy path.
//!
//! Usage:
//!   ssdiag --bar0 0x50000000             # debug dump + FEAT verdict
//!   ssdiag --bar0 0x50000000 --reapply   # + SS0=0x88888888 SS1=0x8 when open
//!
//! Read-back of written values is the only success criterion (const. 1.1).

#![windows_subsystem = "console"]

use sha2::{Digest, Sha256};
use std::ptr;

// ---------------------------------------------------------------- embedded driver

/// In-tree copy of nvoc `core/src/kmd/pmxdrv.sys` (Intel 2019 Authenticode
/// build). Pinned hash must equal nvoc's `PMXDRV_SYS_SHA256`; startup
/// verifies and fails closed on any mismatch.
const PMXDRV_SYS: &[u8] = include_bytes!("pmxdrv.sys");
const PMXDRV_SYS_SHA256: &str =
    "b1a8ee1222eea5f199028d90b9b77c2acf46d6d84a9e125403b2888c6f681c72";

const SERVICE_NAME: &str = "SSDIAG_PMXDRV";
const TEMP_SYS: &str = "ssdiag-pmxdrv.sys";

// ---------------------------------------------------------------- win32

#[allow(non_snake_case)]
mod win {
    use std::ffi::c_void;
    pub type Handle = *mut c_void;
    #[link(name = "advapi32")]
    extern "system" {
        pub fn OpenSCManagerW(
            machine: *const u16,
            db: *const u16,
            access: u32,
        ) -> Handle;
        pub fn OpenServiceW(h: Handle, name: *const u16, access: u32) -> Handle;
        pub fn CreateServiceW(
            h: Handle,
            name: *const u16,
            display: *const u16,
            access: u32,
            svc_type: u32,
            start_type: u32,
            err_type: u32,
            path: *const u16,
            order: *const u16,
            tag: *const u32,
            deps: *const u16,
            account: *const u16,
            password: *const u16,
        ) -> Handle;
        pub fn StartServiceW(h: Handle, argc: u32, argv: *const *const u16) -> i32;
        pub fn ControlService(
            h: Handle,
            code: u32,
            status: *mut ServiceStatus,
        ) -> i32;
        pub fn QueryServiceStatus(h: Handle, status: *mut ServiceStatus) -> i32;
        pub fn DeleteService(h: Handle) -> i32;
        pub fn CloseServiceHandle(h: Handle) -> i32;
    }
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

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct ServiceStatus {
        pub service_type: u32,
        pub current_state: u32,
        pub controls_accepted: u32,
        pub win32_exit: u32,
        pub specific_exit: u32,
        pub checkpoint: u32,
        pub wait_hint: u32,
    }
}

const GENERIC_RW: u32 = 0xC000_0000;
const OPEN_EXISTING: u32 = 3;
const SC_MANAGER_CREATE_SERVICE: u32 = 0x0002;
const SERVICE_ALL_ACCESS: u32 = 0x000F_01FF;
const SERVICE_KERNEL_DRIVER: u32 = 1;
const SERVICE_DEMAND_START: u32 = 3;
const SERVICE_ERROR_NORMAL: u32 = 0;
const SERVICE_CONTROL_STOP: u32 = 1;
const SERVICE_STOPPED: u32 = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_err() -> u32 {
    unsafe { win::GetLastError() }
}

// ---------------------------------------------------------------- pmxdrv transport
// Byte layout copied verbatim from nvoc core/src/kmd/pmxdrv.rs (reverse-
// engineered + live-verified there; the code table was mis-derived once by
// hand - do NOT re-derive).

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
    fn try_open() -> Option<Self> {
        let wide = wide(DEVICE_PATH);
        let h = unsafe {
            win::CreateFileW(
                wide.as_ptr(),
                GENERIC_RW,
                3,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null(),
            )
        };
        if h as isize == -1 {
            None
        } else {
            Some(Self { handle: h })
        }
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
            return Err(last_err());
        }
        Ok(())
    }

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

// ---------------------------------------------------------------- driver lifecycle

fn service_stop_and_wait(svc: win::Handle) -> Result<(), String> {
    let mut st = win::ServiceStatus::default();
    let sent = unsafe { win::ControlService(svc, SERVICE_CONTROL_STOP, &mut st) };
    if sent == 0 && st.current_state != SERVICE_STOPPED {
        return Err(format!("stop refused (win32 {}, state {})", last_err(), st.current_state));
    }
    for _ in 0..25 {
        let mut st = win::ServiceStatus::default();
        if unsafe { win::QueryServiceStatus(svc, &mut st) } != 0 && st.current_state == SERVICE_STOPPED {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err("stop still pending after 5s".into())
}

/// Extract + service-create + start. Service name is OURS (SSDIAG_PMXDRV) so
/// cleanup never touches a driver another tool deployed.
fn deploy_driver() -> Result<(), String> {
    // payload integrity, fail closed
    let mut h = Sha256::new();
    h.update(PMXDRV_SYS);
    let got = format!("{:x}", h.finalize());
    if got != PMXDRV_SYS_SHA256 {
        return Err(format!("embedded pmxdrv.sys hash mismatch ({got}) - refusing to load"));
    }

    let sys_path = std::env::temp_dir().join(TEMP_SYS);
    std::fs::write(&sys_path, PMXDRV_SYS)
        .map_err(|e| format!("extract to {} failed: {e}", sys_path.display()))?;
    let readback = std::fs::read(&sys_path).map_err(|e| format!("re-read failed: {e}"))?;
    if readback != PMXDRV_SYS {
        return Err("extract re-read mismatch (partial write?)".into());
    }

    // ImagePath: \??\ + plain absolute path (canonicalize's \\?\ prefix stacks
    // into an illegal name - nvoc lesson).
    let abs = if sys_path.is_absolute() {
        sys_path
    } else {
        std::env::current_dir().unwrap().join(&sys_path)
    };
    let path_w = wide(format!(r"\??\{}", abs.to_string_lossy()).as_str());
    let name_w = wide(SERVICE_NAME);

    unsafe {
        let mgr = win::OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CREATE_SERVICE);
        if mgr.is_null() {
            return Err(format!(
                "OpenSCManager failed (admin required): win32 {}",
                last_err()
            ));
        }
        // leftover of our own previous run: stop, delete, wait
        let existing = win::OpenServiceW(mgr, name_w.as_ptr(), SERVICE_ALL_ACCESS);
        if !existing.is_null() {
            let _ = service_stop_and_wait(existing);
            win::DeleteService(existing);
            win::CloseServiceHandle(existing);
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        let svc = win::CreateServiceW(
            mgr,
            name_w.as_ptr(),
            name_w.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_KERNEL_DRIVER,
            SERVICE_DEMAND_START,
            SERVICE_ERROR_NORMAL,
            path_w.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
        );
        if svc.is_null() {
            let err = last_err();
            win::CloseServiceHandle(mgr);
            return Err(format!("CreateService failed: win32 {err}"));
        }
        if win::StartServiceW(svc, 0, ptr::null()) == 0 {
            let err = last_err();
            win::DeleteService(svc);
            win::CloseServiceHandle(svc);
            win::CloseServiceHandle(mgr);
            return Err(format!("StartService failed: win32 {err}"));
        }
        win::CloseServiceHandle(svc);
        win::CloseServiceHandle(mgr);
    }
    Ok(())
}

/// Own-the-service cleanup: stop + delete + remove the temp .sys (retry a
/// short while - the kernel releases the image mapping with a small delay).
fn cleanup_driver() {
    unsafe {
        let mgr = win::OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CREATE_SERVICE);
        if mgr.is_null() {
            return;
        }
        let name_w = wide(SERVICE_NAME);
        let svc = win::OpenServiceW(mgr, name_w.as_ptr(), SERVICE_ALL_ACCESS);
        if !svc.is_null() {
            let _ = service_stop_and_wait(svc);
            win::DeleteService(svc);
            win::CloseServiceHandle(svc);
        }
        win::CloseServiceHandle(mgr);
    }
    let sys_path = std::env::temp_dir().join(TEMP_SYS);
    for _ in 0..10 {
        if std::fs::remove_file(&sys_path).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn ensure_driver() -> Result<PmxDrv, String> {
    if let Some(d) = PmxDrv::try_open() {
        println!("PMXDRV device already running - borrowing it (no service changes)");
        return Ok(d);
    }
    deploy_driver()?;
    // the kernel can take a beat between StartService and the device symlink
    for _ in 0..10 {
        if let Some(d) = PmxDrv::try_open() {
            return Ok(d);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    PmxDrv::try_open().ok_or_else(|| "service started but \\\\.\\PMXDRV never appeared".into())
}


// ---------------------------------------------------------------- BAR0 discovery

/// Discover the NVIDIA display adapters' MMIO ranges from the OS itself.
/// Physical assignments change every boot (UEFI assigns one layout, Windows
/// re-assigns another) - a base remembered from any previous session is a
/// hazard, not a shortcut (2026-10-28: a stale UEFI-era address mapped a
/// foreign MMIO window and the first read hard-hung the machine).
fn discover_ranges() -> Result<Vec<(u64, u64)>, String> {
    let ps = "$r = Get-CimInstance Win32_PnPAllocatedResource | Where-Object { $_.Dependent -match 'VEN_10DE' -and $_.Antecedent -match 'Memory' }
              $r | ForEach-Object { Get-CimInstance $_.Antecedent } | ForEach-Object { \"{0} {1}\" -f $_.StartingAddress, $_.EndingAddress }";
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", ps])
        .output()
        .map_err(|e| format!("powershell spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!("powershell query failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let mut ranges = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = line.split_whitespace();
        let (Some(a), Some(b)) = (it.next(), it.next()) else { continue };
        let (Ok(a), Ok(b)) = (a.parse::<u64>(), b.parse::<u64>()) else { continue };
        if a != 0 && b >= a {
            ranges.push((a, b));
        }
    }
    ranges.sort();
    ranges.dedup();
    Ok(ranges)
}

/// Guard rail: the base must be the start of one of the GPU's OWN memory
/// ranges, and after mapping the first register must look like a real
/// PMC_BOOT_0 before anything else is read.
fn validate_base(bar0: u64, ranges: &[(u64, u64)]) -> Result<(), String> {
    println!("GPU memory ranges (this boot):");
    for (a, b) in ranges {
        println!("  {a:#x}-{b:#x} ({} MiB)", (b - a + 1) >> 20);
    }
    if !ranges.iter().any(|(a, _)| *a == bar0) {
        return Err(format!(
            "REFUSED: {bar0:#x} is not the start of any NVIDIA memory range this boot.
             Physical assignments change between UEFI and Windows - never reuse an address from an earlier session or a log."
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- register dump

const SS0_UNLOCK: u32 = 0x8888_8888;
const SS1_UNLOCK: u32 = 0x8;

const FEAT_TURING: [(&str, u32); 3] =
    [("PLM", 0x0040_9650), ("SS0", 0x0040_9664), ("SS1", 0x0040_966C)];
const FEAT_AMPADA: [(&str, u32); 4] = [
    ("PLM", 0x0082_3804),
    ("SS0", 0x0082_381C),
    ("SS1", 0x0082_3820),
    ("READOUT_1", 0x0082_3818),
];
/// Readable-at-PL0 core/debug block. Data-driven on purpose: add rows here
/// as the debug surface grows.
const CORE_ROWS: [(&str, u32); 17] = [
    ("PMC_BOOT_0", 0x0000_0000),
    ("PSTRAPS_SET0", 0x0010_1000),
    ("PSTRAPS_SET1", 0x0010_100C),
    ("FB_SIZE_MIB", 0x0011_83A4),
    ("GFW_PLM", 0x0011_8128),
    ("GFW_SCRATCH", 0x0011_8234),
    ("BSI_SCRATCH15", 0x0011_80FC),
    ("WPR2_LO", 0x001F_A824),
    ("WPR2_HI", 0x001F_A828),
    ("MMU_LOCK_LO", 0x001F_A82C),
    ("MMU_LOCK_HI", 0x001F_A830),
    ("GSP_CPUCTL", 0x0011_0100),
    ("GSP_EXCI", 0x0011_00D0),
    ("SEC2_CPUCTL", 0x0084_0100),
    ("SEC2_MBOX0", 0x0084_0040),
    ("SEC2_MBOX1", 0x0084_0044),
    ("SEC2_EXCI", 0x0084_00D0),
];
/// Privileged block - usually badf at PL0, still worth the record.
const PRIV_ROWS: [(&str, u32); 2] =
    [("FUSE_OPT_FEATURE_OVR", 0x0082_03F0), ("FUSE_VER", 0x0082_4140)];

fn badf_bucket(v: u32) -> Option<&'static str> {
    if v >> 16 != 0xBADF {
        return None;
    }
    Some(match v {
        0xBADF_5040 => "PRIV_VIOLATION",
        0xBADF_1100 => "UNDECODED",
        _ if (v & 0xFF00) == 0x2000 => "FLOORSWEPT",
        _ => "BADF",
    })
}

fn family_of(boot0: u32) -> Option<(&'static str, &'static [(&'static str, u32)])> {
    let die = (boot0 >> 20) & 0xFFF;
    if (0x160..=0x16F).contains(&die) {
        Some(("turing", &FEAT_TURING))
    } else if (0x170..=0x17F).contains(&die) || (0x220..=0x22F).contains(&die) || die == 0x100 {
        Some(("ampere/ada", &FEAT_AMPADA))
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
                // rates 4-7 exist in the field but have no documented meaning
                let rate = match nib & 7 {
                    r @ 0..=3 => names[r as usize],
                    r => return format!("?{r:x}*"),
                };
                format!("{rate}*")
            } else {
                format!("r{}", nib & 7)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn dump_debug(rd: &dyn Fn(u32) -> u32) {
    println!("--- core/debug block ---");
    for (name, off) in CORE_ROWS {
        let v = rd(off);
        match badf_bucket(v) {
            Some(b) => println!("{name:<22} = {v:#010x} [{b}]"),
            None => println!("{name:<22} = {v:#010x}"),
        }
    }
    // PSTRAPS decode (loader sample.rs fields, NvStrapsReBar-verified)
    let s0 = rd(0x0010_1000);
    let s1 = rd(0x0010_100C);
    const BAR1_SIZES: [&str; 11] = [
        "64MiB", "128MiB", "256MiB", "512MiB", "1GiB", "2GiB", "4GiB", "8GiB", "16GiB", "32GiB",
        "64GiB",
    ];
    let bar1_idx = ((s0 >> 14) & 0x3) + ((s1 >> 20) & 0x7);
    let bar0_idx = (s1 >> 17) & 0x7;
    println!(
        "  PSTRAPS: BAR1 index {bar1_idx} = {}   BAR0 = {}MiB   BAR3 = {}xBAR0   override_en = {}",
        BAR1_SIZES.get(bar1_idx as usize).copied().unwrap_or("reserved(>10)"),
        16 << bar0_idx,
        if s1 & 0x0080_0000 != 0 { 1 } else { 2 },
        s0 >> 31
    );
    println!("--- FEAT cluster (both families, cross-visibility) ---");
    for (label, regs) in [
        ("turing 0x4096xx", &FEAT_TURING[..]),
        ("ampere/ada 0x823xxx", &FEAT_AMPADA[..]),
    ] {
        for (name, off) in regs {
            let v = rd(*off);
            let note = match *name {
                "PLM" => format!(" ({})", decode_plm(v)),
                "SS0" => format!(" [{}]", decode_ss(v, 8)),
                "SS1" => format!(" [{}]", decode_ss(v, 1)),
                "READOUT_1" => match badf_bucket(v) {
                    Some(b) => format!(" [{b}]"),
                    None if v == 0 => " [full speed]".into(),
                    None => String::new(),
                },
                _ => String::new(),
            };
            println!("  {label:<20} {name:<9} = {v:#010x}{note}");
        }
    }
    for (name, off) in PRIV_ROWS {
        let v = rd(off);
        let note = badf_bucket(v).map(|b| format!(" [{b}]")).unwrap_or_default();
        println!("{name:<22} = {v:#010x}{note}");
    }
}

fn main() {
    let code = run();
    // strict remove-when-done: our own service and temp file never outlive
    // this process - cleanup runs on every path that got past arg parsing.
    cleanup_driver();
    println!("driver service removed (remove-when-done)");
    std::process::exit(code);
}

fn run() -> i32 {
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
                return 2;
            }
        }
    }
    let ranges = match discover_ranges() {
        Ok(r) if r.is_empty() => {
            eprintln!("no NVIDIA display adapter memory ranges found (driver running?)");
            return 2;
        }
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // auto-pick: exactly one 16 MiB range = BAR0 (straps: 16 MiB << idx)
    let bar0 = match bar0 {
        Some(v) => v,
        None => {
            let cands: Vec<_> = ranges.iter().copied().filter(|(a, b)| b - a + 1 == 0x100_0000).collect();
            match cands.as_slice() {
                [(only, _)] => {
                    println!("BAR0 auto-detected: {only:#x}");
                    *only
                }
                _ => {
                    eprintln!("--bar0 required; GPU memory ranges this boot:");
                    for (a, b) in &ranges {
                        eprintln!("  {a:#x}-{b:#x} ({} MiB)", (b - a + 1) >> 20);
                    }
                    return 2;
                }
            }
        }
    };
    if let Err(e) = validate_base(bar0, &ranges) {
        eprintln!("{e}");
        return 2;
    }

    let drv = match ensure_driver() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };

    const PAGES: u32 = 4096; // 16 MiB BAR0
    let va = match drv.map_physical(bar0, PAGES) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("map BAR0@{bar0:#x} failed: {e}");
            return 1;
        }
    };
    let base = va as *const u8;
    let rd = |off: u32| unsafe { (base.add(off as usize) as *const u32).read_volatile() };
    let wr = |off: u32, v: u32| unsafe { (base.add(off as usize) as *mut u32).write_volatile(v) };

    let boot0 = rd(0x0);
    let fam = family_of(boot0);
    if fam.is_none() {
        // validated against the OS ranges, yet not an NVIDIA PMC - stop here
        drv.unmap_physical(va);
        eprintln!("PMC_BOOT_0={boot0:#010x} does not look like an NVIDIA BAR0 - aborting before any further reads");
        return 1;
    }
    dump_debug(&rd);

    let Some((fam_name, regs)) = fam else {
        eprintln!("PMC_BOOT_0={boot0:#010x} - unsupported chip");
        return 1;
    };
    let get = |name: &str| regs.iter().find(|(n, _)| *n == name).unwrap().1;
    let plm = rd(get("PLM"));
    let ss0 = rd(get("SS0"));
    let ss1 = rd(get("SS1"));
    let unlocked = ss0 == SS0_UNLOCK && ss1 == SS1_UNLOCK;
    println!(
        "VERDICT [{fam_name}]: {}",
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
    rc
}
