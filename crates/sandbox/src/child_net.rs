//! Per-child seccomp network filter. No-op on non-Linux.

#[cfg(any(target_os = "linux", test))]
const X32_SYSCALL_BIT: u32 = 0x4000_0000;
#[cfg(any(target_os = "linux", test))]
const I386_SOCKETCALL_SYSCALL: u32 = 102;

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum SyscallArchitecture {
    X86,
    X86_64,
    Other,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlternateSyscallPolicy {
    None,
    RejectNumberBit(u32),
    RejectSyscall(u32),
}

#[cfg(any(target_os = "linux", test))]
const fn alternate_syscall_policy(architecture: SyscallArchitecture) -> AlternateSyscallPolicy {
    match architecture {
        SyscallArchitecture::X86 => AlternateSyscallPolicy::RejectSyscall(I386_SOCKETCALL_SYSCALL),
        SyscallArchitecture::X86_64 => AlternateSyscallPolicy::RejectNumberBit(X32_SYSCALL_BIT),
        SyscallArchitecture::Other => AlternateSyscallPolicy::None,
    }
}

#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    )
))]
const fn native_alternate_syscall_policy() -> AlternateSyscallPolicy {
    #[cfg(target_arch = "x86")]
    {
        alternate_syscall_policy(SyscallArchitecture::X86)
    }
    #[cfg(target_arch = "x86_64")]
    {
        alternate_syscall_policy(SyscallArchitecture::X86_64)
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        alternate_syscall_policy(SyscallArchitecture::Other)
    }
}

#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    )
))]
const NATIVE_AUDIT_ARCH: u32 = {
    #[cfg(target_arch = "aarch64")]
    {
        0xc000_00b7 // AUDIT_ARCH_AARCH64
    }
    #[cfg(target_arch = "arm")]
    {
        0x4000_0028 // AUDIT_ARCH_ARM
    }
    #[cfg(target_arch = "riscv64")]
    {
        0xc000_00f3 // AUDIT_ARCH_RISCV64
    }
    #[cfg(target_arch = "x86")]
    {
        0x4000_0003 // AUDIT_ARCH_I386
    }
    #[cfg(target_arch = "x86_64")]
    {
        0xc000_003e // AUDIT_ARCH_X86_64 (including x32)
    }
};

#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    )
))]
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    )
))]
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    )
))]
const EPERM_VAL: u32 = 1; // libc::EPERM
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    )
))]
const SYSCALL_NUMBER_OFFSET: u32 = 0; // seccomp_data.nr offset
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    )
))]
const ARCHITECTURE_OFFSET: u32 = 4; // seccomp_data.arch offset

/// Build the seccomp program used for children which must not access the network.
///
/// The architecture check must happen before interpreting `seccomp_data.nr`:
/// syscall numbers are architecture-specific, so dispatching them first could
/// accidentally allow a network syscall under a different ABI.
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    )
))]
fn child_network_filter() -> Vec<libc::sock_filter> {
    use libc::{
        BPF_ABS, BPF_JEQ, BPF_JMP, BPF_JSET, BPF_K, BPF_LD, BPF_RET, BPF_W, SYS_accept,
        SYS_accept4, SYS_bind, SYS_connect, SYS_io_uring_enter, SYS_io_uring_register,
        SYS_io_uring_setup, SYS_listen, SYS_sendmmsg, SYS_sendmsg, SYS_sendto, sock_filter,
    };

    macro_rules! bpf_stmt {
        ($code:expr, $k:expr) => {
            sock_filter {
                code: $code as u16,
                jt: 0,
                jf: 0,
                k: $k as u32,
            }
        };
    }

    macro_rules! bpf_jump {
        ($code:expr, $k:expr, $jt:expr, $jf:expr) => {
            sock_filter {
                code: $code as u16,
                jt: $jt,
                jf: $jf,
                k: $k as u32,
            }
        };
    }

    // io_uring can submit network operations without invoking a conventional
    // network syscall after setup, so block the whole interface conservatively.
    let mut blocked_syscalls = vec![
        SYS_connect as u32,
        SYS_bind as u32,
        SYS_sendto as u32,
        SYS_sendmsg as u32,
        SYS_sendmmsg as u32,
        SYS_listen as u32,
        SYS_accept as u32,
        SYS_accept4 as u32,
        SYS_io_uring_setup as u32,
        SYS_io_uring_enter as u32,
        SYS_io_uring_register as u32,
    ];
    let alternate_policy = native_alternate_syscall_policy();
    if let AlternateSyscallPolicy::RejectSyscall(syscall) = alternate_policy {
        #[cfg(target_arch = "x86")]
        debug_assert_eq!(syscall, libc::SYS_socketcall as u32);
        blocked_syscalls.push(syscall);
    }

    let mut filter = Vec::with_capacity(blocked_syscalls.len() + 7);

    // Reject an ABI we did not build this syscall-number policy for.
    filter.push(bpf_stmt!(BPF_LD | BPF_W | BPF_ABS, ARCHITECTURE_OFFSET));
    filter.push(bpf_jump!(
        BPF_JMP | BPF_JEQ | BPF_K,
        NATIVE_AUDIT_ARCH,
        1,
        0
    ));
    filter.push(bpf_stmt!(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM_VAL));

    filter.push(bpf_stmt!(BPF_LD | BPF_W | BPF_ABS, SYSCALL_NUMBER_OFFSET));
    if let AlternateSyscallPolicy::RejectNumberBit(bit) = alternate_policy {
        filter.push(bpf_jump!(
            BPF_JMP | BPF_JSET | BPF_K,
            bit,
            blocked_syscalls.len() as u8 + 1, // bit set: jump to ERRNO
            0                                 // bit clear: check direct syscalls
        ));
    }
    for (index, &syscall) in blocked_syscalls.iter().enumerate() {
        let remaining_checks = blocked_syscalls.len() - index - 1;
        filter.push(bpf_jump!(
            BPF_JMP | BPF_JEQ | BPF_K,
            syscall,
            remaining_checks as u8 + 1, // match: jump to ERRNO
            0                           // no match: check next
        ));
    }
    filter.push(bpf_stmt!(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
    filter.push(bpf_stmt!(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM_VAL));

    filter
}

#[cfg(test)]
mod structural_policy_tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn x86_64_policy_rejects_x32_syscall_numbers() {
        assert_eq!(
            alternate_syscall_policy(SyscallArchitecture::X86_64),
            AlternateSyscallPolicy::RejectNumberBit(X32_SYSCALL_BIT)
        );
    }

    #[test]
    fn x86_policy_rejects_legacy_socketcall() {
        assert_eq!(
            alternate_syscall_policy(SyscallArchitecture::X86),
            AlternateSyscallPolicy::RejectSyscall(I386_SOCKETCALL_SYSCALL)
        );
    }

    #[test]
    fn other_architectures_have_no_alternate_syscall_abi() {
        assert_eq!(
            alternate_syscall_policy(SyscallArchitecture::Other),
            AlternateSyscallPolicy::None
        );
    }
}

/// Install seccomp BPF filter blocking network syscalls.
///
/// # Safety
///
/// Must be called in a `pre_exec` context (after `fork`, before `exec`).
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    )
))]
pub unsafe fn install_child_network_filter() -> std::io::Result<()> {
    use libc::{PR_SET_NO_NEW_PRIVS, PR_SET_SECCOMP, SECCOMP_MODE_FILTER, prctl, sock_fprog};

    let mut filter = child_network_filter();
    let prog = sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };

    // Must set PR_SET_NO_NEW_PRIVS before applying seccomp filter.
    // SAFETY: prctl with PR_SET_NO_NEW_PRIVS is safe in pre_exec context.
    if unsafe { prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    // SAFETY: prog is a valid sock_fprog pointing to our filter array.
    if unsafe {
        prctl(
            PR_SET_SECCOMP,
            SECCOMP_MODE_FILTER as libc::c_ulong,
            &prog as *const _ as libc::c_ulong,
            0,
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

/// Refuse to claim network restriction where this crate has no verified audit
/// architecture value for the target ABI.
///
/// # Safety
///
/// Must be called in a `pre_exec` context (after `fork`, before `exec`).
#[cfg(all(
    target_os = "linux",
    not(any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    ))
))]
pub unsafe fn install_child_network_filter() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "child network seccomp filter does not support this Linux architecture",
    ))
}

/// # Safety
///
/// No-op on non-Linux.
#[cfg(not(target_os = "linux"))]
pub unsafe fn install_child_network_filter() -> std::io::Result<()> {
    Ok(())
}

#[cfg(all(
    test,
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "x86",
        target_arch = "x86_64"
    )
))]
mod tests {
    use super::*;
    use libc::{BPF_ABS, BPF_JEQ, BPF_JMP, BPF_K, BPF_LD, BPF_RET, BPF_W};
    use pretty_assertions::assert_eq;

    #[test]
    fn policy_checks_architecture_before_syscall_dispatch() {
        let filter = child_network_filter();

        assert_eq!(filter[0].code, (BPF_LD | BPF_W | BPF_ABS) as u16);
        assert_eq!(filter[0].k, ARCHITECTURE_OFFSET);
        assert_eq!(filter[1].code, (BPF_JMP | BPF_JEQ | BPF_K) as u16);
        assert_eq!(filter[1].k, NATIVE_AUDIT_ARCH);
        assert_eq!(filter[1].jt, 1);
        assert_eq!(filter[1].jf, 0);
        assert_eq!(filter[2].code, (BPF_RET | BPF_K) as u16);
        assert_eq!(filter[2].k, SECCOMP_RET_ERRNO | EPERM_VAL);
        assert_eq!(filter[3].code, (BPF_LD | BPF_W | BPF_ABS) as u16);
        assert_eq!(filter[3].k, SYSCALL_NUMBER_OFFSET);
    }

    #[test]
    fn policy_blocks_batched_and_io_uring_network_paths() {
        let filter = child_network_filter();
        let syscall_checks: Vec<u32> = filter
            .iter()
            .filter(|instruction| {
                instruction.code == (BPF_JMP | BPF_JEQ | BPF_K) as u16
                    && instruction.k != NATIVE_AUDIT_ARCH
            })
            .map(|instruction| instruction.k)
            .collect();

        for syscall in [
            libc::SYS_sendmmsg,
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
        ] {
            assert!(
                syscall_checks.contains(&(syscall as u32)),
                "missing syscall policy for {syscall}"
            );
        }

        assert_eq!(filter[filter.len() - 2].k, SECCOMP_RET_ALLOW);
        assert_eq!(filter[filter.len() - 1].k, SECCOMP_RET_ERRNO | EPERM_VAL);
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn generated_policy_rejects_x32_before_direct_syscall_dispatch() {
        use libc::BPF_JSET;

        let filter = child_network_filter();

        assert_eq!(filter[4].code, (BPF_JMP | BPF_JSET | BPF_K) as u16);
        assert_eq!(filter[4].k, X32_SYSCALL_BIT);
        assert_eq!(filter[4].jf, 0);
    }

    #[test]
    #[cfg(target_arch = "x86")]
    fn generated_policy_blocks_legacy_socketcall() {
        let filter = child_network_filter();
        let syscall_checks: Vec<u32> = filter
            .iter()
            .filter(|instruction| {
                instruction.code == (BPF_JMP | BPF_JEQ | BPF_K) as u16
                    && instruction.k != NATIVE_AUDIT_ARCH
            })
            .map(|instruction| instruction.k)
            .collect();

        assert!(syscall_checks.contains(&(libc::SYS_socketcall as u32)));
    }
}
