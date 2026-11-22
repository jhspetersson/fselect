const CAP_NAMES: [&str; 41] = [
    "cap_chown",
    "cap_dac_override",
    "cap_dac_read_search",
    "cap_fowner",
    "cap_fsetid",
    "cap_kill",
    "cap_setgid",
    "cap_setuid",
    "cap_setpcap",
    "cap_linux_immutable",
    "cap_net_bind_service",
    "cap_net_broadcast",
    "cap_net_admin",
    "cap_net_raw",
    "cap_ipc_lock",
    "cap_ipc_owner",
    "cap_sys_module",
    "cap_sys_rawio",
    "cap_sys_chroot",
    "cap_sys_ptrace",
    "cap_sys_pacct",
    "cap_sys_admin",
    "cap_sys_boot",
    "cap_sys_nice",
    "cap_sys_resource",
    "cap_sys_time",
    "cap_sys_tty_config",
    "cap_mknod",
    "cap_lease",
    "cap_audit_write",
    "cap_audit_control",
    "cap_setfcap",
    "cap_mac_override",
    "cap_mac_admin",
    "cap_syslog",
    "cap_wake_alarm",
    "cap_block_suspend",
    "cap_audit_read",
    "cap_perfmon",
    "cap_bpf",
    "cap_checkpoint_restore",
];

const VFS_CAP_REVISION_MASK: u32 = 0xFF000000;
const VFS_CAP_REVISION_1: u32 = 0x01000000;
const VFS_CAP_REVISION_2: u32 = 0x02000000;
const VFS_CAP_REVISION_3: u32 = 0x03000000;
const VFS_CAP_FLAGS_EFFECTIVE: u32 = 0x000001;
const XATTR_CAPS_SZ_1: usize = 12; // 4 (magic) + 4 (permitted) + 4 (inherited)
const XATTR_CAPS_SZ_2: usize = 20; // 4 (magic) + 2 * (4 (permitted) + 4 (inherited))
const XATTR_CAPS_SZ_3: usize = 24; // v2 + 4 (rootid)

pub fn read_capabilities(path: &std::path::Path) -> Result<Option<Vec<u8>>, std::io::Error> {
    xattr::get(path, "security.capability")
}

pub fn parse_capabilities(caps: Vec<u8>) -> String {
    if caps.len() < 4 {
        return String::new();
    }

    let word = |index: usize| u32::from_le_bytes(caps[index * 4..index * 4 + 4].try_into().unwrap());
    let magic_etc = word(0);
    let effective = if magic_etc & VFS_CAP_FLAGS_EFFECTIVE != 0 { "e" } else { "" };

    let (permitted, inherited, rootid) = match magic_etc & VFS_CAP_REVISION_MASK {
        VFS_CAP_REVISION_1 if caps.len() >= XATTR_CAPS_SZ_1 => (word(1) as u64, word(2) as u64, 0),
        VFS_CAP_REVISION_2 | VFS_CAP_REVISION_3 if caps.len() >= XATTR_CAPS_SZ_2 => (
            word(1) as u64 | (word(3) as u64) << 32,
            word(2) as u64 | (word(4) as u64) << 32,
            if caps.len() >= XATTR_CAPS_SZ_3 { word(5) } else { 0 },
        ),
        _ => return String::new(),
    };

    let mut result: Vec<String> = vec![];

    for bit in 0..64 {
        let flags = match (permitted >> bit & 1, inherited >> bit & 1) {
            (1, 1) => "ip",
            (1, 0) => "p",
            (0, 1) => "i",
            _ => continue,
        };
        match CAP_NAMES.get(bit) {
            Some(name) => result.push(format!("{}={}{}", name, effective, flags)),
            None => result.push(format!("{}={}{}", bit, effective, flags)),
        }
    }

    if result.is_empty() {
        result.push(String::from("="));
    }

    if rootid != 0 {
        result.push(format!("[rootid={}]", rootid));
    }

    result.join(" ")
}

pub fn has_capability(caps_string: &str, cap_name: &str) -> bool {
    !cap_name.is_empty()
        && caps_string.split_whitespace().any(|entry| {
            entry.split('=').next().is_some_and(|name| {
                name.eq_ignore_ascii_case(cap_name)
                    || name.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case("cap_"))
                        && name[4..].eq_ignore_ascii_case(cap_name)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2_blob(magic_flags: u32, permitted0: u32, inherited0: u32) -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend_from_slice(&(VFS_CAP_REVISION_2 | magic_flags).to_le_bytes());
        blob.extend_from_slice(&permitted0.to_le_bytes());
        blob.extend_from_slice(&inherited0.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob
    }

    #[test]
    fn test_parse_v2_blob_as_written_by_setcap() {
        // `setcap cap_net_bind_service=ep` writes a v2 blob whose magic is
        // 0x02000000 | effective flag; parsing must not come back empty.
        let blob = v2_blob(VFS_CAP_FLAGS_EFFECTIVE, 1 << 10, 0);
        assert_eq!(parse_capabilities(blob), "cap_net_bind_service=ep");
    }

    #[test]
    fn test_parse_v3_blob_with_rootid() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&(VFS_CAP_REVISION_3 | VFS_CAP_FLAGS_EFFECTIVE).to_le_bytes());
        blob.extend_from_slice(&(1u32 << 10).to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&1000u32.to_le_bytes());
        assert_eq!(
            parse_capabilities(blob),
            "cap_net_bind_service=ep [rootid=1000]"
        );
    }

    #[test]
    fn test_parse_v2_blob_without_effective_flag() {
        let blob = v2_blob(0, 1 << 21, 0);
        assert_eq!(parse_capabilities(blob), "cap_sys_admin=p");
    }

    #[test]
    fn test_has_capability_exact_match() {
        let caps = "cap_net_bind_service=ep cap_net_admin=ep";
        assert!(has_capability(caps, "cap_net_bind_service"));
        assert!(has_capability(caps, "cap_net_admin"));
    }

    #[test]
    fn test_has_capability_no_substring_match() {
        let caps = "cap_net_bind_service=ep cap_net_admin=ep";
        // Should NOT match via substring
        assert!(!has_capability(caps, "cap_net"));
        assert!(!has_capability(caps, "cap_net_bind"));
    }

    #[test]
    fn test_has_capability_empty() {
        assert!(!has_capability("", "cap_net_admin"));
    }

    #[test]
    fn test_has_capability_single() {
        assert!(has_capability("cap_sys_admin=ep", "cap_sys_admin"));
        assert!(!has_capability("cap_sys_admin=ep", "cap_sys"));
    }

    #[test]
    fn test_parse_reports_unknown_bits_by_number() {
        let mut blob = v2_blob(VFS_CAP_FLAGS_EFFECTIVE, 1 << 13, 0);
        blob[12..16].copy_from_slice(&(1u32 << 20).to_le_bytes());
        assert_eq!(parse_capabilities(blob), "cap_net_raw=ep 52=ep");
    }

    #[test]
    fn test_parse_empty_set_like_libcap() {
        assert_eq!(parse_capabilities(v2_blob(0, 0, 0)), "=");
        assert_eq!(parse_capabilities(v2_blob(VFS_CAP_FLAGS_EFFECTIVE, 0, 0)), "=");
        assert!(!has_capability("=", ""));
        assert!(!has_capability("=", "cap_chown"));
    }

    #[test]
    fn test_parse_mixed_flags_and_second_word() {
        let mut blob = v2_blob(VFS_CAP_FLAGS_EFFECTIVE, 1 << 5 | 1 << 13, 1 | 1 << 5);
        blob[12..16].copy_from_slice(&(1u32 << 8).to_le_bytes());
        assert_eq!(
            parse_capabilities(blob),
            "cap_chown=ei cap_kill=eip cap_net_raw=ep cap_checkpoint_restore=ep"
        );
    }

    #[test]
    fn test_parse_v1_blob() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&(VFS_CAP_REVISION_1 | VFS_CAP_FLAGS_EFFECTIVE).to_le_bytes());
        blob.extend_from_slice(&(1u32 << 13).to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse_capabilities(blob), "cap_net_raw=ep");
    }

    #[test]
    fn test_has_capability_ignores_case() {
        assert!(has_capability("cap_net_raw=ep 52=ep", "CAP_NET_RAW"));
        assert!(has_capability("cap_net_raw=ep 52=ep", "52"));
    }

    #[test]
    fn test_has_capability_accepts_names_without_prefix() {
        assert!(has_capability("cap_net_raw=ep cap_chown=p", "net_raw"));
        assert!(has_capability("cap_net_raw=ep cap_chown=p", "CHOWN"));
        assert!(!has_capability("cap_net_raw=ep", "net"));
        assert!(!has_capability("cap_net_raw=ep", "raw"));
    }
}
