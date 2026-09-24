//! The guest side of Wispers Access, shared by every client app: joining and
//! restoring shares over any transport, the local share store, the guest API
//! calls, and the loopback HTTP proxy.
//!
//! Step 1 of the SDK: waclient's modules, moved here unchanged. The `Client`
//! API comes next (see `docs/access/client-api.md` in wispers).

pub mod http;
pub mod iroh_transport;
pub mod shares;
pub mod storage;
pub mod transports;
pub mod wispers_connect_transport;
