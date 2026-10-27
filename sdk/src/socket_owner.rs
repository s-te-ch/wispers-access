//! Socket ownership checks, used to check that a process talking to one of our
//! local ports belongs to the same user as this process.

// Compiled in every OS's tests too, for the test of its table parser.
#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::net::SocketAddr;

/// Whether the process that connected from `peer` to our `local` address
/// belongs to this OS user. An error if that can't be told, which callers
/// take as no.
pub fn is_same_user(local: SocketAddr, peer: SocketAddr) -> std::io::Result<bool> {
    #[cfg(target_os = "linux")]
    return linux::is_same_user(local, peer);
    #[cfg(target_os = "macos")]
    return macos::is_same_user(local, peer);
    #[cfg(windows)]
    return windows::is_same_user(local, peer);
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (local, peer);
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};

    /// A connection this process makes to itself is this user's.
    fn connects_as_this_user(listen_on: SocketAddr) {
        let listener = TcpListener::bind(listen_on).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, peer) = listener.accept().unwrap();
        assert_eq!(peer, client.local_addr().unwrap());
        assert!(is_same_user(server.local_addr().unwrap(), peer).unwrap());
    }

    #[test]
    fn this_process_is_this_user_on_ipv4() {
        connects_as_this_user((Ipv4Addr::LOCALHOST, 0).into());
    }

    #[test]
    fn this_process_is_this_user_on_ipv6() {
        if std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).is_err() {
            eprintln!("no IPv6 loopback on this machine");
            return;
        }
        connects_as_this_user((Ipv6Addr::LOCALHOST, 0).into());
    }

    #[test]
    fn a_connection_that_does_not_exist_is_nobodys() {
        let ours: SocketAddr = (Ipv4Addr::LOCALHOST, 1).into();
        let theirs: SocketAddr = (Ipv4Addr::LOCALHOST, 2).into();
        assert!(!matches!(is_same_user(ours, theirs), Ok(true)));
    }
}
