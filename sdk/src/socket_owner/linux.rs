//! Linux: `/proc/net/tcp` and `/proc/net/tcp6` list every TCP socket with the
//! user ID of its owner.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Only on Linux, since the other platforms compile this module just for the
/// parser's tests, and Windows has no `libc`.
#[cfg(target_os = "linux")]
pub fn is_same_user(local: SocketAddr, peer: SocketAddr) -> std::io::Result<bool> {
    use std::io;

    let table = match peer {
        SocketAddr::V4(_) => "/proc/net/tcp",
        SocketAddr::V6(_) => "/proc/net/tcp6",
    };
    let owner = owner_uid(&std::fs::read_to_string(table)?, local, peer)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such connection"))?;
    // SAFETY: geteuid has no preconditions and can't fail.
    Ok(owner == unsafe { libc::geteuid() })
}

/// The user ID of the socket whose own end is `peer` and whose far end is
/// `local`, in a table formatted as `/proc/net/tcp`:
/// `sl local_address rem_address st tx_queue:rx_queue tr:tm->when retrnsmt uid …`.
fn owner_uid(table: &str, local: SocketAddr, peer: SocketAddr) -> Option<u32> {
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (own, far, uid) = (fields.get(1)?, fields.get(2)?, fields.get(7)?);
        (parse_addr(own)? == peer && parse_addr(far)? == local).then(|| uid.parse().ok())?
    })
}

/// `0100007F:1F90`: the address as the kernel's native-endian 32-bit words in
/// hex (one for IPv4, four for IPv6), a colon, and the port in hex.
fn parse_addr(field: &str) -> Option<SocketAddr> {
    let (ip, port) = field.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let words = (0..ip.len() / 8)
        .map(|i| u32::from_str_radix(ip.get(i * 8..i * 8 + 8)?, 16).ok())
        .collect::<Option<Vec<u32>>>()?;
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_ne_bytes()).collect();
    let ip = match bytes.len() {
        4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(bytes).ok()?)),
        16 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(bytes).ok()?)),
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    // On a little-endian machine, as the tests run; the kernel writes the
    // words in the machine's own order.
    #[cfg(target_endian = "little")]
    #[test]
    fn sockets_are_found_in_the_table() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1
   1: 0100007F:C350 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1001        0 2 1";
        let local: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        assert_eq!(owner_uid(table, local, peer), Some(1001));
        assert_eq!(owner_uid(table, peer, local), None);
        assert_eq!(
            parse_addr("00000000000000000000000001000000:1F90"),
            Some("[::1]:8080".parse().unwrap())
        );
    }
}
