//! Windows: the TCP table names the process that owns each connection, and the
//! process's token its user.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

pub fn is_same_user(local: SocketAddr, peer: SocketAddr) -> io::Result<bool> {
    let owner = owning_pid(local, peer)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such connection"))?;
    // SAFETY: GetCurrentProcessId has no preconditions and can't fail.
    let us = unsafe { GetCurrentProcessId() };
    Ok(user_of(owner)? == user_of(us)?)
}

/// The process that owns the connection from `peer` to `local`, from the
/// TCP table of the connection's address family.
fn owning_pid(local: SocketAddr, peer: SocketAddr) -> io::Result<Option<u32>> {
    let family = if peer.is_ipv4() { AF_INET } else { AF_INET6 };
    let table = tcp_table(family as u32)?;
    // SAFETY: the buffer holds the table GetExtendedTcpTable wrote for this
    // family: a row count followed by that many rows.
    let found = unsafe {
        if peer.is_ipv4() {
            let table = table.as_ptr() as *const MIB_TCPTABLE_OWNER_PID;
            let rows: &[MIB_TCPROW_OWNER_PID] =
                std::slice::from_raw_parts((*table).table.as_ptr(), (*table).dwNumEntries as usize);
            rows.iter()
                .find(|row| {
                    v4(row.dwLocalAddr, row.dwLocalPort) == peer
                        && v4(row.dwRemoteAddr, row.dwRemotePort) == local
                })
                .map(|row| row.dwOwningPid)
        } else {
            let table = table.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID;
            let rows: &[MIB_TCP6ROW_OWNER_PID] =
                std::slice::from_raw_parts((*table).table.as_ptr(), (*table).dwNumEntries as usize);
            rows.iter()
                .find(|row| {
                    v6(row.ucLocalAddr, row.dwLocalPort) == peer
                        && v6(row.ucRemoteAddr, row.dwRemotePort) == local
                })
                .map(|row| row.dwOwningPid)
        }
    };
    Ok(found)
}

/// The TCP table of one address family with each connection's owning
/// process, in a buffer aligned for its rows.
fn tcp_table(family: u32) -> io::Result<Vec<u64>> {
    let mut size = 0u32;
    loop {
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: the buffer is `size` bytes long, or `size` is 0 and the
        // call only reports how big it must be.
        let result = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &mut size,
                0,
                family,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        match result {
            NO_ERROR => return Ok(buffer),
            // The table grew since the size was asked for; ask again.
            ERROR_INSUFFICIENT_BUFFER => continue,
            code => return Err(io::Error::from_raw_os_error(code as i32)),
        }
    }
}

/// An IPv4 end as the table holds it: both in network order, the port in the
/// low 16 bits.
fn v4(addr: u32, port: u32) -> SocketAddr {
    SocketAddr::new(
        IpAddr::V4(Ipv4Addr::from(u32::from_be(addr))),
        u16::from_be(port as u16),
    )
}

fn v6(addr: [u8; 16], port: u32) -> SocketAddr {
    SocketAddr::new(IpAddr::V6(Ipv6Addr::from(addr)), u16::from_be(port as u16))
}

/// The security identifier (SID) of the user the process runs as, as bytes.
fn user_of(pid: u32) -> io::Result<Vec<u8>> {
    // SAFETY: a failed call returns a null handle, checked before use.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(process);
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `process` is open, `token` is a valid place for the result.
    if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    let mut size = 0u32;
    // SAFETY: a null buffer of size 0 only asks for the size needed.
    unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut size) };
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: the buffer is `size` bytes long and aligned for TOKEN_USER.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the buffer now holds a TOKEN_USER, whose SID points into it.
    unsafe {
        let sid = (*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid;
        let len = GetLengthSid(sid) as usize;
        Ok(std::slice::from_raw_parts(sid as *const u8, len).to_vec())
    }
}

/// Closes the handle when dropped.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is open and owned by this value.
        unsafe { CloseHandle(self.0) };
    }
}
