macro_rules! check_cap {
    ($cap_name: ident, $code: expr, $permitted: ident, $inherited: ident, $effective: ident, $result: ident) => {
        if let Some(str_result) = check_capability($permitted, $inherited, 1 << $code) {
            $result.push(stringify!($cap_name).to_owned() + "=" + &$effective + &str_result);
        }
    };
}

macro_rules! check_caps_word_0 {
    ($permitted: ident, $inherited: ident, $effective: ident, $result: ident) => {
        check_cap!(cap_chown, 0, $permitted, $inherited, $effective, $result);
        check_cap!(cap_dac_override, 1, $permitted, $inherited, $effective, $result);
        check_cap!(cap_dac_read_search, 2, $permitted, $inherited, $effective, $result);
        check_cap!(cap_fowner, 3, $permitted, $inherited, $effective, $result);
        check_cap!(cap_fsetid, 4, $permitted, $inherited, $effective, $result);
        check_cap!(cap_kill, 5, $permitted, $inherited, $effective, $result);
        check_cap!(cap_setgid, 6, $permitted, $inherited, $effective, $result);
        check_cap!(cap_setuid, 7, $permitted, $inherited, $effective, $result);
        check_cap!(cap_setpcap, 8, $permitted, $inherited, $effective, $result);
        check_cap!(cap_linux_immutable, 9, $permitted, $inherited, $effective, $result);
        check_cap!(cap_net_bind_service, 10, $permitted, $inherited, $effective, $result);
        check_cap!(cap_net_broadcast, 11, $permitted, $inherited, $effective, $result);
        check_cap!(cap_net_admin, 12, $permitted, $inherited, $effective, $result);
        check_cap!(cap_net_raw, 13, $permitted, $inherited, $effective, $result);
        check_cap!(cap_ipc_lock, 14, $permitted, $inherited, $effective, $result);
        check_cap!(cap_ipc_owner, 15, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_module, 16, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_rawio, 17, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_chroot, 18, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_ptrace, 19, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_pacct, 20, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_admin, 21, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_boot, 22, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_nice, 23, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_resource, 24, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_time, 25, $permitted, $inherited, $effective, $result);
        check_cap!(cap_sys_tty_config, 26, $permitted, $inherited, $effective, $result);
        check_cap!(cap_mknod, 27, $permitted, $inherited, $effective, $result);
        check_cap!(cap_lease, 28, $permitted, $inherited, $effective, $result);
        check_cap!(cap_audit_write, 29, $permitted, $inherited, $effective, $result);
        check_cap!(cap_audit_control, 30, $permitted, $inherited, $effective, $result);
        check_cap!(cap_setfcap, 31, $permitted, $inherited, $effective, $result);
    };
}

macro_rules! check_caps_word_1 {
    ($permitted: ident, $inherited: ident, $effective: ident, $result: ident) => {
        check_cap!(cap_mac_override, 0, $permitted, $inherited, $effective, $result);
        check_cap!(cap_mac_admin, 1, $permitted, $inherited, $effective, $result);
        check_cap!(cap_syslog, 2, $permitted, $inherited, $effective, $result);
        check_cap!(cap_wake_alarm, 3, $permitted, $inherited, $effective, $result);
        check_cap!(cap_block_suspend, 4, $permitted, $inherited, $effective, $result);
        check_cap!(cap_audit_read, 5, $permitted, $inherited, $effective, $result);
        check_cap!(cap_perfmon, 6, $permitted, $inherited, $effective, $result);
        check_cap!(cap_bpf, 7, $permitted, $inherited, $effective, $result);
        check_cap!(cap_checkpoint_restore, 8, $permitted, $inherited, $effective, $result);
    };
}

const VFS_CAP_REVISION_1: u32 = 0x01000000;
const VFS_CAP_REVISION_2: u32 = 0x02000002;
const VFS_CAP_FLAGS_EFFECTIVE: u32 = 0x000001;
const XATTR_CAPS_SZ_1: usize = 12; // 4 (magic) + 4 (permitted) + 4 (inherited)
const XATTR_CAPS_SZ_2: usize = 20; // 4 (magic) + 2 * (4 (permitted) + 4 (inherited))
const XATTR_CAPS_SZ_3: usize = 24; // v2 + 4 (rootid)

pub fn parse_capabilities(caps: Vec<u8>) -> String {
    if caps.len() < 4 {
        return String::new();
    }

    let magic_etc = u32::from_le_bytes(caps[0..4].try_into().unwrap());
    let revision = magic_etc & !VFS_CAP_FLAGS_EFFECTIVE;

    let effective = if magic_etc & VFS_CAP_FLAGS_EFFECTIVE != 0 {
        String::from("e")
    } else {
        String::new()
    };

    let mut result: Vec<String> = vec![];

    match revision {
        VFS_CAP_REVISION_1 => {
            if caps.len() < XATTR_CAPS_SZ_1 {
                return String::new();
            }

            let permitted = u32::from_le_bytes(caps[4..8].try_into().unwrap());
            let inherited = u32::from_le_bytes(caps[8..12].try_into().unwrap());

            check_caps_word_0!(permitted, inherited, effective, result);
        }
        VFS_CAP_REVISION_2 => {
            // v2 (20 bytes) and v3 (24 bytes with rootid) share the same revision
            if caps.len() < XATTR_CAPS_SZ_2 {
                return String::new();
            }

            let permitted = u32::from_le_bytes(caps[4..8].try_into().unwrap());
            let inherited = u32::from_le_bytes(caps[8..12].try_into().unwrap());

            check_caps_word_0!(permitted, inherited, effective, result);

            let permitted = u32::from_le_bytes(caps[12..16].try_into().unwrap());
            let inherited = u32::from_le_bytes(caps[16..20].try_into().unwrap());

            check_caps_word_1!(permitted, inherited, effective, result);

            // v3 has a rootid (namespace owner UID) appended
            if caps.len() >= XATTR_CAPS_SZ_3 {
                let rootid = u32::from_le_bytes(caps[20..24].try_into().unwrap());
                if rootid != 0 {
                    result.push(format!("[rootid={}]", rootid));
                }
            }
        }
        _ => return String::new(),
    }

    result.join(" ")
}

fn check_capability(perm: u32, inh: u32, cap: u32) -> Option<String> {
    if inh & cap == cap && perm & cap == cap {
        Some(String::from("ip"))
    } else if perm & cap == cap {
        Some(String::from("p"))
    } else if inh & cap == cap {
        Some(String::from("i"))
    } else {
        None
    }
}

/// Check if a capabilities string contains a specific capability by exact name match.
/// The capabilities string is space-separated entries like "cap_net_bind_service=ep cap_net_admin=ep".
pub fn has_capability(caps_string: &str, cap_name: &str) -> bool {
    caps_string.split_whitespace().any(|entry| {
        match entry.find('=') {
            Some(idx) => &entry[..idx] == cap_name,
            None => entry == cap_name,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
