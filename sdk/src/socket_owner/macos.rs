//! macOS: no call maps a socket to its process, so this walks the sockets of
//! this user's processes for the one whose ends match. A connection found in
//! none of them is another user's. A browser opens nearly all its connections
//! from one network process, so the process that owned the last one is tried
//! first.

use libproc::libproc::bsd_info::BSDInfo;
use libproc::libproc::file_info::{ListFDs, ProcFDType, pidfdinfo};
use libproc::libproc::net_info::{InSIAddr, SocketFDInfo, SocketInfoKind};
use libproc::libproc::proc_pid::{listpidinfo, pidinfo};
use libproc::processes::{ProcFilter, pids_by_type};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicI32, Ordering};

/// The process that owned the last connection found, 0 for none yet.
static LAST_OWNER: AtomicI32 = AtomicI32::new(0);

pub fn is_same_user(local: SocketAddr, peer: SocketAddr) -> io::Result<bool> {
    let last = LAST_OWNER.load(Ordering::Relaxed);
    if last != 0 && owns(last, local, peer) {
        return Ok(true);
    }
    // SAFETY: getuid has no preconditions and can't fail.
    let uid = unsafe { libc::getuid() };
    for pid in pids_by_type(ProcFilter::ByUID { uid })? {
        let pid = pid as i32;
        if pid != last && owns(pid, local, peer) {
            LAST_OWNER.store(pid, Ordering::Relaxed);
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether one of `pid`'s sockets is the connection from `peer` to `local`.
/// A process that ended meanwhile, or can't be inspected, owns nothing.
fn owns(pid: i32, local: SocketAddr, peer: SocketAddr) -> bool {
    let Ok(info) = pidinfo::<BSDInfo>(pid, 0) else {
        return false;
    };
    let Ok(fds) = listpidinfo::<ListFDs>(pid, info.pbi_nfiles as usize) else {
        return false;
    };
    fds.iter()
        .filter(|fd| matches!(ProcFDType::from(fd.proc_fdtype), ProcFDType::Socket))
        .any(|fd| {
            let Ok(socket) = pidfdinfo::<SocketFDInfo>(pid, fd.proc_fd) else {
                return false;
            };
            if socket.psi.soi_kind != SocketInfoKind::Tcp as i32 {
                return false;
            }
            // SAFETY: soi_kind says the union holds TCP socket info.
            let ends = unsafe { socket.psi.soi_proto.pri_tcp.tcpsi_ini };
            let own = socket_end(&ends.insi_laddr, ends.insi_lport, ends.insi_vflag);
            let far = socket_end(&ends.insi_faddr, ends.insi_fport, ends.insi_vflag);
            own == Some(peer) && far == Some(local)
        })
}

/// One end of a socket as `in_sockinfo` holds it: the port in network order
/// in the low 16 bits, the address IPv4 or IPv6 as `vflag` says.
fn socket_end(addr: &InSIAddr, port: i32, vflag: u8) -> Option<SocketAddr> {
    const INI_IPV4: u8 = 0x1;
    const INI_IPV6: u8 = 0x2;
    let port = u16::from_be(port as u16);
    // SAFETY: vflag says which member of the union is set.
    let ip = unsafe {
        if vflag & INI_IPV6 != 0 {
            IpAddr::V6(Ipv6Addr::from(addr.ina_6.s6_addr))
        } else if vflag & INI_IPV4 != 0 {
            IpAddr::V4(Ipv4Addr::from(u32::from_be(addr.ina_46.i46a_addr4.s_addr)))
        } else {
            return None;
        }
    };
    Some(SocketAddr::new(ip, port))
}
