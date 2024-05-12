#[cfg(target_os = "linux")]
macro_rules! check_cap {
    ($cap_name: ident, $code: expr, $permitted: ident, $inherited: ident, $effective: ident, $result: ident) => {
        if let Some(str_result) = check_capability($permitted, $inherited, 1 << $code) {
            $result.push(stringify!($cap_name).to_owned() + "=" + &$effective + &str_result);
        }
    };
}

#[cfg(target_os = "linux")]
pub fn parse_capabilities(caps: Vec<u8>) -> String {
    if caps.len() < 12 {
        return String::new();
    }

    let mut result: Vec<String> = vec![];

    let effective = if caps[0] == 1 {
        String::from("e")
    } else {
        String::new()
    };

    let permitted = u32::from_le_bytes(caps[4..8].try_into().unwrap());
    let inherited = u32::from_le_bytes(caps[8..12].try_into().unwrap());

    check_cap!(cap_chown, 0, permitted, inherited, effective, result);
    check_cap!(cap_dac_override, 1, permitted, inherited, effective, result);
    check_cap!(
        cap_dac_read_search,
        2,
        permitted,
        inherited,
        effective,
        result
    );
    check_cap!(cap_fowner, 3, permitted, inherited, effective, result);
    check_cap!(cap_fsetid, 4, permitted, inherited, effective, result);
    check_cap!(cap_kill, 5, permitted, inherited, effective, result);
    check_cap!(cap_setgid, 6, permitted, inherited, effective, result);
    check_cap!(cap_setuid, 7, permitted, inherited, effective, result);
    check_cap!(cap_setpcap, 8, permitted, inherited, effective, result);
    check_cap!(
        cap_linux_immutable,
        9,
        permitted,
        inherited,
        effective,
        result
    );
    check_cap!(
        cap_net_bind_service,
        10,
        permitted,
        inherited,
        effective,
        result
    );
    check_cap!(
        cap_net_broadcast,
        11,
        permitted,
        inherited,
        effective,
        result
    );
    check_cap!(cap_net_admin, 12, permitted, inherited, effective, result);
    check_cap!(cap_net_raw, 13, permitted, inherited, effective, result);
    check_cap!(cap_ipc_lock, 14, permitted, inherited, effective, result);
    check_cap!(cap_ipc_owner, 15, permitted, inherited, effective, result);
    check_cap!(cap_sys_module, 16, permitted, inherited, effective, result);
    check_cap!(cap_sys_rawio, 17, permitted, inherited, effective, result);
    check_cap!(cap_sys_chroot, 18, permitted, inherited, effective, result);
    check_cap!(cap_sys_ptrace, 19, permitted, inherited, effective, result);
    check_cap!(cap_sys_pacct, 20, permitted, inherited, effective, result);
    check_cap!(cap_sys_admin, 21, permitted, inherited, effective, result);
    check_cap!(cap_sys_boot, 22, permitted, inherited, effective, result);
    check_cap!(cap_sys_nice, 23, permitted, inherited, effective, result);
    check_cap!(
        cap_sys_resource,
        24,
        permitted,
        inherited,
        effective,
        result
    );
    check_cap!(cap_sys_time, 25, permitted, inherited, effective, result);
    check_cap!(
        cap_sys_tty_config,
        26,
        permitted,
        inherited,
        effective,
        result
    );
    check_cap!(cap_mknod, 27, permitted, inherited, effective, result);
    check_cap!(cap_lease, 28, permitted, inherited, effective, result);
    check_cap!(cap_audit_write, 29, permitted, inherited, effective, result);
    check_cap!(
        cap_audit_control,
        30,
        permitted,
        inherited,
        effective,
        result
    );
    check_cap!(cap_setfcap, 31, permitted, inherited, effective, result);

    if caps.len() >= 20 {
        let permitted = u32::from_le_bytes(caps[12..16].try_into().unwrap());
        let inherited = u32::from_le_bytes(caps[16..20].try_into().unwrap());

        check_cap!(
            cap_mac_override,
            32 - 32,
            permitted,
            inherited,
            effective,
            result
        );
        check_cap!(
            cap_mac_admin,
            33 - 32,
            permitted,
            inherited,
            effective,
            result
        );
        check_cap!(cap_syslog, 34 - 32, permitted, inherited, effective, result);
        check_cap!(
            cap_wake_alarm,
            35 - 32,
            permitted,
            inherited,
            effective,
            result
        );
        check_cap!(
            cap_block_suspend,
            36 - 32,
            permitted,
            inherited,
            effective,
            result
        );
        check_cap!(
            cap_audit_read,
            37 - 32,
            permitted,
            inherited,
            effective,
            result
        );
        check_cap!(
            cap_perfmon,
            38 - 32,
            permitted,
            inherited,
            effective,
            result
        );
        check_cap!(cap_bpf, 39 - 32, permitted, inherited, effective, result);
        check_cap!(
            cap_checkpoint_restore,
            40 - 32,
            permitted,
            inherited,
            effective,
            result
        );
    }

    result.join(" ")
}

#[cfg(target_os = "linux")]
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
