use super::InterfaceFinder;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) use linux::set_so_mark;

cfg_if::cfg_if! {
    if #[cfg(any(target_os = "linux", target_os = "android"))] {
        pub(crate) use linux::set_interface;
    } else if #[cfg(target_os = "windows")] {
        pub(crate) use windows::set_interface;
    } else if #[cfg(target_os = "macos")] {
        pub(crate) use macos::set_interface;
    } else {
        pub(crate) async fn set_interface<F>(
            _f: &F,
            _finder: &super::InterfaceFinder,
            _iface: &str,
            _is_ipv6: bool
        ) -> std::io::Result<()> {
            Err(std::io::Error::new(std::io::ErrorKind::Unsupported, format!("unsupported platform")))
        }
    }
}

//

#[cfg(any(target_os = "linux", target_os = "android"))]
mod linux {
    use std::{io, mem, os::unix::io::AsRawFd};

    pub(crate) fn set_so_mark<F: AsRawFd>(f: &F, so_mark: u32) -> io::Result<()> {
        let mark = so_mark.to_be();
        let ret = unsafe {
            libc::setsockopt(
                f.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_MARK,
                &mark as *const _ as *const _,
                mem::size_of_val(&mark) as libc::socklen_t,
            )
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(crate) async fn set_interface<F: AsRawFd>(
        f: &F,
        _finder: &super::InterfaceFinder,
        iface: &str,
        _is_ipv6: bool,
    ) -> io::Result<()> {
        let iface_bytes = iface.as_bytes();

        let ret = unsafe {
            libc::setsockopt(
                f.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_BINDTODEVICE,
                iface_bytes.as_ptr() as *const _ as *const libc::c_void,
                iface_bytes.len() as libc::socklen_t,
            )
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(any(target_os = "windows"))]
mod windows {
    use std::{io, mem, os::windows::io::AsRawSocket};

    use winapi::{
        shared::{
            ws2def::{IPPROTO_IP, IPPROTO_IPV6},
            ws2ipdef::IPV6_UNICAST_IF,
        },
        um::{
            winnt::PCSTR,
            winsock2::{htonl, setsockopt, WSAGetLastError, SOCKET, SOCKET_ERROR},
        },
    };

    const IP_UNICAST_IF: u32 = 31;

    pub(crate) async fn set_interface<F: AsRawSocket>(
        f: &F,
        finder: &super::InterfaceFinder,
        iface: &str,
        is_ipv6: bool,
    ) -> io::Result<()> {
        let handle = f.as_raw_socket() as SOCKET;

        let iface_idx = finder.find_id_by_tag(iface).await.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("interface [{}] not found", iface),
            )
        })?;

        let ret = unsafe {
            let idx = htonl(iface_idx);
            if !is_ipv6 {
                setsockopt(
                    handle,
                    IPPROTO_IP as i32,
                    IP_UNICAST_IF as i32,
                    &idx as *const _ as PCSTR,
                    mem::size_of_val(&idx) as i32,
                )
            } else {
                setsockopt(
                    handle,
                    IPPROTO_IPV6 as i32,
                    IPV6_UNICAST_IF as i32,
                    &idx as *const _ as PCSTR,
                    mem::size_of_val(&idx) as i32,
                )
            }
        };

        if ret == SOCKET_ERROR {
            let err = io::Error::from_raw_os_error(unsafe { WSAGetLastError() });
            return Err(err);
        }

        Ok(())
    }
}

#[cfg(any(target_os = "macos"))]
mod macos {
    use std::{io, mem, os::unix::io::AsRawFd};

    const IP_BOUND_IF: libc::c_int = 25; // bsd/netinet/in.h
    const IPV6_BOUND_IF: libc::c_int = 125; // bsd/netinet6/in6.h

    pub(crate) async fn set_interface<F: AsRawFd>(
        f: &F,
        finder: &super::InterfaceFinder,
        iface: &str,
        is_ipv6: bool,
    ) -> io::Result<()> {
        let iface_idx = finder.find_id_by_tag(iface).await.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("interface [{}] not found", iface),
            )
        })?;

        let ret = unsafe {
            if !is_ipv6 {
                libc::setsockopt(
                    f.as_raw_fd(),
                    libc::IPPROTO_IP,
                    IP_BOUND_IF,
                    &iface_idx as *const _ as *const _,
                    mem::size_of_val(&iface_idx) as libc::socklen_t,
                )
            } else {
                libc::setsockopt(
                    f.as_raw_fd(),
                    libc::IPPROTO_IPV6,
                    IPV6_BOUND_IF,
                    &iface_idx as *const _ as *const _,
                    mem::size_of_val(&iface_idx) as libc::socklen_t,
                )
            }
        };

        if ret < 0 {
            let err = io::Error::last_os_error();
            return Err(err);
        }

        Ok(())
    }
}
