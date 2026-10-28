//! UEFI device-path ownership match: reference implementation + shape tests.
//!
//! `loader/src/tx_detach.rs::path_belongs_to` must stay textually identical
//! to [`path_belongs_to`] here (loader is UEFI-target-only and cannot run
//! host tests; sim is the test bench).
//!
//! The shapes below are the real-world instances, not inventions:
//! - the 28/36-byte dGPU/GOP pair from the TU106 1f07 regression log of
//!   2026-10-28 (GOP-TOPOLOGY: 36 B present, gone after the GPU PCI
//!   disconnect, back after reconnect) - the raw byte-prefix compare judged
//!   the dGPU's own GOP "foreign";
//! - the issue #14 flip board and the ASUS Z790 2803 report, same mechanism;
//! - the multi-GPU negative cases the check exists for (other cards' GOPs
//!   must stay untouched so their consoles stay alive).

/// The loader's ownership test. Parent's node sequence (its END excluded)
/// must appear byte-exact at the same offsets in the child. A malformed
/// node (length < 4 or past the buffer) ends the parent walk.
pub fn path_belongs_to(parent: &[u8], child: &[u8]) -> bool {
    let mut body = 0usize;
    let mut nodes = 0usize;
    let mut i = 0usize;
    while i + 4 <= parent.len() {
        let len = u16::from_le_bytes([parent[i + 2], parent[i + 3]]) as usize;
        if len < 4 || i + len > parent.len() || parent[i] == 0x7f {
            break;
        }
        body = i + len;
        nodes += 1;
        i += len;
    }
    if nodes == 0 {
        return false;
    }
    child.len() >= body && child[..body] == parent[..body]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ACPI HID node (type 0x02, subtype 0x01, 12 bytes).
    fn acpi(hid: u32, uid: u32) -> Vec<u8> {
        let mut n = vec![0x02, 0x01, 0x0c, 0x00];
        n.extend_from_slice(&hid.to_le_bytes());
        n.extend_from_slice(&uid.to_le_bytes());
        n
    }

    /// PCI node (type 0x01, subtype 0x01, 6 bytes).
    fn pci(dev: u8, func: u8) -> Vec<u8> {
        vec![0x01, 0x01, 0x06, 0x00, dev, func]
    }

    /// ACPI ADR node (type 0x82, subtype 0x01, 8 bytes) - the display node
    /// NVIDIA GOP child handles carry in place of the PCI path's END node.
    fn acpi_adr(adr: u32) -> Vec<u8> {
        let mut n = vec![0x82, 0x01, 0x08, 0x00];
        n.extend_from_slice(&adr.to_le_bytes());
        n
    }

    /// END_ENTIRE node.
    fn end() -> Vec<u8> {
        vec![0x7f, 0xff, 0x04, 0x00]
    }

    fn chain(nodes: &[&[u8]]) -> Vec<u8> {
        nodes.concat()
    }

    /// dGPU 01:00.0 PCI handle path: ACPI + Pci(0x1,0) + Pci(0x0,0) + END.
    fn dgpu_parent() -> Vec<u8> {
        chain(&[&acpi(0x0a03, 0), &pci(0x01, 0x00), &pci(0x00, 0x00), &end()])
    }

    #[test]
    fn tu106_regression_shape_gop_child_with_acpi_adr() {
        // The exact shape from the 2026-10-28 TU106 log: the raw byte-prefix
        // compare failed because child[24..28] is the AcpiAdr header where
        // the parent has its END node.
        let parent = dgpu_parent();
        let gop = chain(&[
            &acpi(0x0a03, 0),
            &pci(0x01, 0x00),
            &pci(0x00, 0x00),
            &acpi_adr(0),
            &end(),
        ]);
        assert_eq!(parent.len(), 28);
        assert_eq!(gop.len(), 36);
        assert!(path_belongs_to(&parent, &gop));
        // The old raw byte-prefix compare, for the record:
        assert!(!(gop.len() >= parent.len() && gop[..parent.len()] == parent[..]));
    }

    #[test]
    fn identical_path_belongs() {
        let parent = dgpu_parent();
        assert!(path_belongs_to(&parent, &parent.clone()));
    }

    #[test]
    fn multi_node_extension_belongs() {
        let parent = dgpu_parent();
        let gop = chain(&[
            &acpi(0x0a03, 0),
            &pci(0x01, 0x00),
            &pci(0x00, 0x00),
            &acpi_adr(0),
            &pci(0xff, 0xfe),
            &end(),
        ]);
        assert!(path_belongs_to(&parent, &gop));
    }

    #[test]
    fn other_gpu_gop_stays_foreign() {
        // Second dGPU at 04:00.0 - the multi-GPU case the check exists for.
        let parent = dgpu_parent();
        let other = chain(&[
            &acpi(0x0a03, 0),
            &pci(0x04, 0x00),
            &pci(0x00, 0x00),
            &acpi_adr(0),
            &end(),
        ]);
        assert!(!path_belongs_to(&parent, &other));
    }

    #[test]
    fn igpu_gop_stays_foreign() {
        // iGPU at 00:02.0 (8086:3e9b on the TU106 regression machine).
        let parent = dgpu_parent();
        let igpu = chain(&[&acpi(0x0a03, 0), &pci(0x02, 0x00), &acpi_adr(0), &end()]);
        assert!(!path_belongs_to(&parent, &igpu));
    }

    #[test]
    fn child_shorter_than_parent_is_foreign() {
        let parent = dgpu_parent();
        let truncated = chain(&[&acpi(0x0a03, 0), &pci(0x01, 0x00), &end()]);
        assert!(!path_belongs_to(&parent, &truncated));
    }

    #[test]
    fn pathless_parent_is_foreign() {
        assert!(!path_belongs_to(&end(), &dgpu_parent()));
        assert!(!path_belongs_to(&[], &dgpu_parent()));
    }

    #[test]
    fn malformed_parent_node_ends_walk() {
        // A node declaring length 2 is malformed; the walk stops before it
        // and the complete-node prefix (here: nothing) is unmatched.
        let broken = vec![0x01, 0x01, 0x02, 0x00, 0x00, 0x00];
        assert!(!path_belongs_to(&broken, &dgpu_parent()));
        // One good node then a malformed one: the good prefix still owns a
        // child carrying it.
        let semi = chain(&[&pci(0x01, 0x00), &broken, &end()]);
        let child = chain(&[&pci(0x01, 0x00), &pci(0x00, 0x00), &end()]);
        assert!(path_belongs_to(&semi, &child));
    }

    #[test]
    fn tampered_middle_byte_is_foreign() {
        let parent = dgpu_parent();
        let mut tampered = chain(&[
            &acpi(0x0a03, 0),
            &pci(0x01, 0x00),
            &pci(0x00, 0x00),
            &acpi_adr(0),
            &end(),
        ]);
        tampered[13] ^= 0x01; // flip a byte inside the first PCI node
        assert!(!path_belongs_to(&parent, &tampered));
    }
}
