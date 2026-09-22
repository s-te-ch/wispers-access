//! The `status` command.
//!
//! Overview with `waserver status`, per-share status with
//! `waserver status <share>`. Both forms gather into the same serializable
//! report, so `--json` and the human rendering never disagree.

use crate::config::{AppKind, ShareConfig, TransportConfig};
use crate::ipc;
use crate::iroh_transport;
use crate::storage;
use crate::wcbe;
use crate::wispers_connect_transport;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::io::Write;
use std::time::Duration;
use tabwriter::TabWriter;

const UPSTREAM_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

pub async fn run(share: Option<&str>, json: bool) -> Result<()> {
    let report = match share {
        Some(share) => {
            if !storage::ShareDir::new(share)?.exists() {
                anyhow::bail!("Share {} is not initialised", share);
            }
            let loaded = load_share(share).map_err(|e| format!("{:#}", e));
            StatusReport {
                shares: vec![gather_share(share, loaded).await],
                groups_quota: None,
            }
        }
        None => gather_fleet().await?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if share.is_some() {
        print_share_details(&report.shares[0]);
    } else {
        print_fleet(&report);
    }
    Ok(())
}

//-- Report shape (the `--json` contract) --------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusReport {
    shares: Vec<ShareStatus>,
    /// Domain-wide connectivity-group quota, one entry per distinct
    /// backend + API key among the shares. Fleet view only; omitted when
    /// no stats query succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    groups_quota: Option<Vec<GroupsQuota>>,
}

/// One backend's connectivity-group quota usage (see `groupsQuota`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupsQuota {
    /// Custom backend URL, `null` for the managed backend.
    backend: Option<String>,
    /// The local shares whose API key counts against this quota. Two
    /// shares with different keys live in different domains — each gets its
    /// own entry, even on the same backend.
    shares: Vec<String>,
    /// The API key's public ID part (`wc_<env>_<id>`, everything before the
    /// dot — the secret half is never shown). Matches the key listing in the
    /// backend's console. `null` when the key doesn't have the expected shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
    count: i32,
    /// `null` = unlimited.
    max: Option<i32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareStatus {
    name: String,
    /// Display name from `share.toml`, `null` when the file failed to load
    /// (see `configError`).
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_error: Option<String>,
    /// The transport and what is specific to it; `null` when the config
    /// failed to load.
    transport: Option<TransportStatus>,
    server: ServerStatus,
    /// The apps: the running server's list while it runs, else the file's.
    apps: Vec<AppStatus>,
    /// The guests; the host itself is not listed. `null` when the query
    /// failed (see `guestsError`).
    guests: Option<Vec<GuestStatus>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    guests_error: Option<String>,
    /// `null` when the query failed (see `invitesError`).
    invites: Option<Vec<InviteStatus>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    invites_error: Option<String>,
}

/// What only one transport has, tagged by `kind` as in `share.toml`.
#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum TransportStatus {
    WispersConnect {
        /// Custom backend URL, `null` for the managed backend.
        backend: Option<String>,
        connectivity_group_id: Option<String>,
        group_created_at: Option<String>, // RFC 3339
        /// Node-quota usage of the group: `current` (this host node, guest
        /// nodes, and pending invites) vs `limit` (`null` = unlimited). Omitted
        /// when the backend doesn't report it.
        #[serde(skip_serializing_if = "Option::is_none")]
        node_quota: Option<wcbe::NodeQuota>,
    },
    Iroh {
        /// The host node's endpoint ID.
        endpoint_id: Option<String>,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerStatus {
    /// `serving` | `connecting` | `offline` | `error`
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Reachable by guests.
    reachable: Option<bool>,
    pid: Option<u32>,
    started_at: Option<String>,      // RFC 3339
    connected_since: Option<String>, // RFC 3339
    /// `share.toml` on disk differs from what the server serves. `null` when
    /// the server is down or the file is broken.
    reload_pending: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppStatus {
    id: String,
    name: String,
    kind: AppKind,
    upstream: String,
    /// A TCP connection to the upstream succeeded just now.
    upstream_reachable: bool,
}

/// A recent invite. An invite showing `used` with no matching guest means the
/// join was rolled back — issue a new invite.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InviteStatus {
    pub(crate) node_name: Option<String>,
    pub(crate) user_id: Option<String>,
    pub(crate) created_at: String,      // RFC 3339
    pub(crate) expires_at: String,      // RFC 3339
    pub(crate) used_at: Option<String>, // RFC 3339
    /// `pending` | `used` | `expired`
    pub(crate) status: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GuestStatus {
    /// The node number (Wispers Connect) or guest number (iroh).
    pub(crate) node_number: i32,
    pub(crate) name: Option<String>,
    pub(crate) user_id: Option<String>,
    pub(crate) created_at: String,           // RFC 3339
    pub(crate) last_seen_at: Option<String>, // RFC 3339
    /// Does the guest have a live P2P connection to this host node right now?
    pub(crate) connected_to_server: Option<bool>,
    pub(crate) connected_since: Option<String>, // RFC 3339
    /// The host node revoked the guest; its key is never served again. A guest
    /// that left is simply gone. iroh only: the hub's integrator API does not
    /// expose the roster's node state, and a revoked node is deregistered there
    /// anyway.
    pub(crate) revoked: bool,
    /// When, where known (iroh; the hub's roster carries no time).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) revoked_at: Option<String>, // RFC 3339
}

//-- Gathering -----------------------------------------------------------------

async fn gather_fleet() -> Result<StatusReport> {
    let mut names = storage::list_shares()?;
    names.sort();
    // Load every share once, up front; the two gathers below share the result.
    let loaded: Vec<(String, Result<Loaded, String>)> = names
        .into_iter()
        .map(|name| {
            let l = load_share(&name).map_err(|e| format!("{:#}", e));
            (name, l)
        })
        .collect();
    let (shares, groups_quota) = tokio::join!(gather_shares(&loaded), gather_groups_quota(&loaded));
    Ok(StatusReport {
        shares: shares?,
        groups_quota,
    })
}

async fn gather_shares(loaded: &[(String, Result<Loaded, String>)]) -> Result<Vec<ShareStatus>> {
    // Query all shares concurrently.
    let mut tasks = tokio::task::JoinSet::new();
    for (i, (name, l)) in loaded.iter().enumerate() {
        let name = name.clone();
        let l = l.clone();
        tasks.spawn(async move { (i, gather_share(&name, l).await) });
    }
    let mut shares: Vec<Option<ShareStatus>> = loaded.iter().map(|_| None).collect();
    while let Some(joined) = tasks.join_next().await {
        let (i, share) = joined.context("status task failed")?;
        shares[i] = Some(share);
    }
    Ok(shares.into_iter().flatten().collect())
}

/// Queries `GET /stats` once per distinct backend + API key among the
/// shares. The group count is domain-wide, so it can exceed the number of
/// local shares (other machines and integrations mint into the same
/// domain). `None` when no query succeeded (connectivity trouble already
/// surfaces per share as `guestsError`).
async fn gather_groups_quota(
    loaded: &[(String, Result<Loaded, String>)],
) -> Option<Vec<GroupsQuota>> {
    struct Target {
        api_base: String,
        api_key: String,
        backend: Option<String>,
        shares: Vec<String>,
    }
    let mut targets: Vec<Target> = Vec::new();
    for (name, l) in loaded {
        let Ok(Loaded {
            config,
            wcs: Some(wcs),
            ..
        }) = l
        else {
            continue;
        };
        let TransportConfig::WispersConnect { backend } = &config.transport else {
            continue;
        };
        let api_base = wcbe::api_base(backend.as_deref());
        match targets
            .iter_mut()
            .find(|t| t.api_base == api_base && t.api_key == wcs.api_key)
        {
            Some(t) => t.shares.push(name.clone()),
            None => targets.push(Target {
                api_base,
                api_key: wcs.api_key.clone(),
                backend: backend.clone(),
                shares: vec![name.clone()],
            }),
        }
    }
    let mut quotas = Vec::new();
    for t in targets {
        let client = wcbe::Client::new(&t.api_key, &t.api_base);
        if let Ok(stats) = client.get_stats().await {
            quotas.push(GroupsQuota {
                backend: t.backend,
                shares: t.shares,
                key_id: wcbe::key_id(&t.api_key).map(str::to_owned),
                count: stats.connectivity_groups.count,
                max: stats.connectivity_groups.max,
            });
        }
    }
    (!quotas.is_empty()).then_some(quotas)
}

async fn gather_share(name: &str, loaded: Result<Loaded, String>) -> ShareStatus {
    let (loaded, config_error) = match loaded {
        Ok(l) => (Some(l), None),
        Err(e) => (None, Some(e)),
    };
    let config = loaded.as_ref().map(|l| &l.config);
    let wcs = loaded.as_ref().and_then(|l| l.wcs.as_ref());
    let (server, report) = tokio::join!(
        query_server(name, config),
        query_transport(config, wcs, loaded.as_ref().and_then(|l| l.state.as_ref()))
    );
    let (server, live_guests, served_apps) = server;
    let (mut guests, guests_error) = match report.guests {
        Ok(g) => (Some(g), None),
        Err(e) => (None, Some(e)),
    };
    let (invites, invites_error) = match report.invites {
        Ok(i) => (Some(i), None),
        Err(e) => (None, Some(e)),
    };
    match (guests.as_mut(), live_guests.as_ref()) {
        (Some(guests), Some(connected)) => apply_live_connections(guests, connected),
        // A stopped server has no connections.
        (Some(guests), None) if server.state == "offline" => {
            for g in guests.iter_mut() {
                g.connected_to_server = Some(false);
            }
        }
        _ => {}
    }
    // The running server's app list wins; a stopped server shows the file's.
    let apps = match (served_apps, config) {
        (Some(apps), _) => apps,
        (None, Some(cfg)) => cfg
            .apps
            .iter()
            .map(|s| ipc::AppData {
                id: s.id.clone(),
                name: s.name.clone(),
                kind: s.kind,
                upstream: s.upstream.clone(),
            })
            .collect(),
        (None, None) => Vec::new(),
    };
    let apps = probe_apps(apps).await;
    ShareStatus {
        name: name.to_owned(),
        display_name: config.map(|c| c.name.clone()),
        config_error,
        transport: report.transport,
        server,
        apps,
        guests,
        guests_error,
        invites,
        invites_error,
    }
}

/// The transport's part of the status report.
pub(crate) struct TransportReport {
    pub(crate) guests: Result<Vec<GuestStatus>, String>,
    pub(crate) invites: Result<Vec<InviteStatus>, String>,
    pub(crate) transport: Option<TransportStatus>,
}

async fn query_transport(
    config: Option<&ShareConfig>,
    wcs: Option<&storage::WispersConnectState>,
    state: Option<&storage::StateDb>,
) -> TransportReport {
    match config.map(|c| &c.transport) {
        Some(TransportConfig::WispersConnect { backend }) => {
            wispers_connect_transport::report(backend.as_deref(), wcs).await
        }
        Some(TransportConfig::Iroh {}) => iroh_transport::report(state),
        None => {
            let err = "share config failed to load";
            TransportReport {
                guests: Err(err.to_owned()),
                invites: Err(err.to_owned()),
                transport: None,
            }
        }
    }
}

#[derive(Clone)]
struct Loaded {
    config: ShareConfig,
    wcs: Option<storage::WispersConnectState>,
    state: Option<storage::StateDb>,
}

/// A config that fails to load is an error; a missing `state.db` (an `init`
/// that did not complete) only leaves `wcs` empty, so the apps still show.
fn load_share(name: &str) -> Result<Loaded> {
    let dir = storage::ShareDir::new(name)?;
    let config = dir.load_config()?;
    let state = match dir.open_state() {
        Ok(state) => Some(state),
        Err(storage::Error::NotInitialised(_)) => None,
        Err(e) => return Err(e.into()),
    };
    let wcs = match &state {
        Some(state) => state.wispers_connect_state()?,
        None => None,
    };
    Ok(Loaded { config, wcs, state })
}

/// Overlay the server's live view onto the guest list: while the daemon
/// runs it knows authoritatively which guests are connected to it.
fn apply_live_connections(guests: &mut [GuestStatus], connected: &[ipc::GuestData]) {
    for g in guests.iter_mut().filter(|g| !g.revoked) {
        match connected.iter().find(|c| c.node_number == g.node_number) {
            Some(c) => {
                g.connected_to_server = Some(true);
                g.connected_since = c.connected_since.clone();
            }
            None => g.connected_to_server = Some(false),
        }
    }
}

/// Queries the daemon. The second element is its connected-guest list, the third
/// the app list it serves (both `None` when the daemon is down).
async fn query_server(
    share: &str,
    config: Option<&ShareConfig>,
) -> (
    ServerStatus,
    Option<Vec<ipc::GuestData>>,
    Option<Vec<ipc::AppData>>,
) {
    let Ok(mut client) = ipc::Client::connect(share).await else {
        return (ServerStatus::offline(), None, None);
    };
    match client.request(&ipc::Request::Status).await {
        Ok(ipc::Response::Success {
            data: ipc::ResponseData::Status(s),
            ..
        }) => {
            let status = ServerStatus {
                state: if s.reachable { "serving" } else { "connecting" },
                error: None,
                reachable: Some(s.reachable),
                pid: s.pid,
                started_at: s.started_at,
                connected_since: s.connected_since,
                reload_pending: config.map(|c| c.config_hash() != s.config_hash),
            };
            (status, s.connected_guests, Some(s.apps))
        }
        Ok(ipc::Response::Success { .. }) => (
            ServerStatus::error("unexpected response from server"),
            None,
            None,
        ),
        Ok(ipc::Response::Error { error, .. }) => (ServerStatus::error(error), None, None),
        // Probably went down just now.
        Err(_) => (ServerStatus::offline(), None, None),
    }
}

impl ServerStatus {
    fn offline() -> Self {
        Self {
            state: "offline",
            error: None,
            reachable: None,
            pid: None,
            started_at: None,
            connected_since: None,
            reload_pending: None,
        }
    }

    fn error(msg: impl Into<String>) -> Self {
        Self {
            state: "error",
            error: Some(msg.into()),
            ..Self::offline()
        }
    }
}

/// Probes every app's upstream concurrently.
async fn probe_apps(apps: Vec<ipc::AppData>) -> Vec<AppStatus> {
    let mut probes = tokio::task::JoinSet::new();
    for (i, s) in apps.iter().enumerate() {
        let upstream = s.upstream.clone();
        probes.spawn(async move { (i, probe_upstream(&upstream).await) });
    }
    let mut reachable = vec![false; apps.len()];
    while let Some(Ok((i, r))) = probes.join_next().await {
        reachable[i] = r;
    }
    apps.into_iter()
        .zip(reachable)
        .map(|(s, upstream_reachable)| AppStatus {
            id: s.id,
            name: s.name,
            kind: s.kind,
            upstream: s.upstream,
            upstream_reachable,
        })
        .collect()
}

/// True if a TCP connection to the upstream succeeds.
async fn probe_upstream(upstream: &str) -> bool {
    tokio::time::timeout(
        UPSTREAM_PROBE_TIMEOUT,
        tokio::net::TcpStream::connect(upstream),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

pub(crate) fn invite_status(
    used_at: Option<&str>,
    expires_at: &str,
    now: DateTime<Utc>,
) -> &'static str {
    if used_at.is_some() {
        return "used";
    }
    match parse_rfc3339(expires_at) {
        Some(expiry) if expiry <= now => "expired",
        _ => "pending",
    }
}

//-- Human rendering -----------------------------------------------------------

fn print_fleet(report: &StatusReport) {
    if report.shares.is_empty() {
        println!("No shares found");
        return;
    }
    let mut tw = TabWriter::new(std::io::stdout().lock()).padding(2);
    writeln!(&mut tw, "SHARE\tSTATE\tNETWORK\tAPPS\tNODES").unwrap();
    for c in &report.shares {
        let hub = match c.server.reachable {
            Some(true) => "connected",
            Some(false) => "not connected",
            None => "-",
        };
        let apps = if c.apps.is_empty() {
            "-".to_owned()
        } else {
            c.apps
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let nodes = match &c.guests {
            // Live server view: connected guests / total guests.
            Some(m) if m.iter().any(|m| m.connected_to_server.is_some()) => {
                let connected = m
                    .iter()
                    .filter(|m| m.connected_to_server == Some(true))
                    .count();
                let guests = m.iter().filter(|m| m.connected_to_server.is_some()).count();
                format!("{}/{} connected", connected, guests)
            }
            Some(g) => format!("{} guests", g.len()),
            None => "?".to_owned(),
        };
        let state = match (c.server.state, c.server.reload_pending) {
            (s, Some(true)) => format!("{} (reload pending)", s),
            (s, _) => s.to_owned(),
        };
        writeln!(
            &mut tw,
            "{}\t{}\t{}\t{}\t{}",
            c.name, state, hub, apps, nodes
        )
        .unwrap();
    }
    tw.flush().unwrap();

    if let Some(quotas) = &report.groups_quota {
        println!("\nShare usage per API key");
        for q in quotas {
            let limit = match q.max {
                Some(max) => max.to_string(),
                None => "∞".to_owned(),
            };
            println!(
                "   {} ({}): {} of {}",
                q.key_id.as_deref().unwrap_or("unknown key"),
                q.shares.join(", "),
                q.count,
                limit
            );
        }
    }
}

fn print_share_details(c: &ShareStatus) {
    let mut tw = TabWriter::new(std::io::stdout().lock()).padding(2);

    writeln!(&mut tw, "Share").unwrap();
    let name = match &c.display_name {
        Some(dn) => format!("{} ({})", c.name, dn),
        None => c.name.clone(),
    };
    writeln!(&mut tw, "  Name\t{}", name).unwrap();
    if let Some(e) = &c.config_error {
        writeln!(&mut tw, "  Config\tERROR: {}", e).unwrap();
    }
    match &c.transport {
        Some(TransportStatus::WispersConnect {
            backend,
            connectivity_group_id,
            group_created_at,
            node_quota,
        }) => {
            writeln!(&mut tw, "  Transport\twispers-connect").unwrap();
            writeln!(&mut tw, "  Backend\t{}", backend_label(backend.as_deref())).unwrap();
            writeln!(
                &mut tw,
                "  Connectivity group\t{}",
                connectivity_group_id.as_deref().unwrap_or("-")
            )
            .unwrap();
            if let Some(created) = group_created_at {
                writeln!(&mut tw, "  Created\t{}", fmt_utc(created)).unwrap();
            }
            if let Some(quota) = node_quota {
                let guests = c.guests.as_ref().map(|g| g.len()).unwrap_or(0);
                writeln!(&mut tw, "  Quota\t{}", fmt_quota(quota, guests)).unwrap();
            }
        }
        Some(TransportStatus::Iroh { endpoint_id }) => {
            writeln!(&mut tw, "  Transport\tiroh").unwrap();
            writeln!(
                &mut tw,
                "  Endpoint ID\t{}",
                endpoint_id.as_deref().unwrap_or("-")
            )
            .unwrap();
        }
        None => writeln!(&mut tw, "  Transport\t-").unwrap(),
    }

    writeln!(&mut tw, "\nServer").unwrap();
    let server = &c.server;
    let mut state = server.state.to_owned();
    if let (Some(pid), Some(started)) = (server.pid, server.started_at.as_deref()) {
        state = format!("{} (pid {}, up {})", state, pid, fmt_age(started));
    }
    if let Some(e) = &server.error {
        state = format!("{}: {}", state, e);
    }
    writeln!(&mut tw, "  State\t{}", state).unwrap();
    if let Some(connected) = server.reachable {
        let hub = match (connected, server.connected_since.as_deref()) {
            (true, Some(since)) => format!("connected (for {})", fmt_age(since)),
            (true, None) => "connected".to_owned(),
            (false, _) => "not connected".to_owned(),
        };
        let label = match c.transport {
            Some(TransportStatus::Iroh { .. }) => "Relay",
            _ => "Hub",
        };
        writeln!(&mut tw, "  {}\t{}", label, hub).unwrap();
    }
    if server.reload_pending == Some(true) {
        writeln!(
            &mut tw,
            "  Config\tchanged on disk; run `waserver reload {}`",
            c.name
        )
        .unwrap();
    }

    writeln!(&mut tw, "\nApps").unwrap();
    if c.apps.is_empty() {
        writeln!(&mut tw, "  (none configured)").unwrap();
    } else {
        writeln!(&mut tw, "  ID\tNAME\tUPSTREAM\tKIND").unwrap();
        for s in &c.apps {
            let reachable = if s.upstream_reachable {
                " (reachable)"
            } else {
                " (unreachable!)"
            };
            writeln!(
                &mut tw,
                "  {}\t{}\t{}{}\t{}",
                s.id,
                s.name,
                s.upstream,
                reachable,
                s.kind.as_str()
            )
            .unwrap();
        }
    }

    writeln!(&mut tw, "\nGuests").unwrap();
    match (&c.guests, &c.guests_error) {
        (Some(guests), _) => {
            writeln!(&mut tw, "  #\tNAME\tUSER\tLAST SEEN\tSTATUS").unwrap();
            for g in guests {
                let last_seen = if g.connected_to_server == Some(true) {
                    "now".to_owned()
                } else {
                    match g.last_seen_at.as_deref() {
                        Some(at) => fmt_ago(at),
                        None => "-".to_owned(),
                    }
                };
                let status = match (g.revoked, &g.revoked_at, g.connected_to_server) {
                    (true, Some(at), _) => format!("revoked ({})", fmt_ago(at)),
                    (true, None, _) => "revoked".to_owned(),
                    (false, _, Some(true)) => match &g.connected_since {
                        Some(since) => format!("connected ({})", fmt_age(since)),
                        None => "connected".to_owned(),
                    },
                    (false, _, Some(false)) => "-".to_owned(),
                    (false, _, None) => "?".to_owned(),
                };
                writeln!(
                    &mut tw,
                    "  {}\t{}\t{}\t{}\t{}",
                    g.node_number,
                    g.name.as_deref().unwrap_or("-"),
                    g.user_id.as_deref().unwrap_or("-"),
                    last_seen,
                    status
                )
                .unwrap();
            }
            if matches!(c.transport, Some(TransportStatus::WispersConnect { .. })) {
                writeln!(
                    &mut tw,
                    "  (node {} is the host)",
                    wispers_connect_transport::SERVER_NODE_NUMBER
                )
                .unwrap();
            }
        }
        (None, Some(e)) => writeln!(&mut tw, "  (unavailable: {})", e).unwrap(),
        (None, None) => writeln!(&mut tw, "  (unavailable)").unwrap(),
    }

    writeln!(&mut tw, "\nInvites").unwrap();
    match (&c.invites, &c.invites_error) {
        (Some(invites), _) if invites.is_empty() => {
            writeln!(&mut tw, "  (none in the last 7 days)").unwrap()
        }
        (Some(invites), _) => {
            writeln!(&mut tw, "  NODE NAME\tUSER\tCREATED\tSTATUS").unwrap();
            for i in invites {
                let status = match i.status {
                    "pending" => format!("pending (expires in {})", fmt_until(&i.expires_at)),
                    other => other.to_owned(),
                };
                writeln!(
                    &mut tw,
                    "  {}\t{}\t{}\t{}",
                    i.node_name.as_deref().unwrap_or("-"),
                    i.user_id.as_deref().unwrap_or("-"),
                    fmt_ago(&i.created_at),
                    status
                )
                .unwrap();
            }
        }
        (None, Some(e)) => writeln!(&mut tw, "  (unavailable: {})", e).unwrap(),
        (None, None) => writeln!(&mut tw, "  (unavailable)").unwrap(),
    }

    tw.flush().unwrap();
}

/// Renders node-quota usage, e.g. `11 of 12 used (this host node + 8 guest
/// nodes + 2 pending invites)`. The hub counts the host as one node of the
/// group.
fn fmt_quota(quota: &wcbe::NodeQuota, guest_count: usize) -> String {
    let used = fmt_used(quota.current, quota.limit);
    let pending = (quota.current.max(0) as usize).saturating_sub(guest_count + 1);
    if pending == 0 {
        return used;
    }
    format!(
        "{} (this host + {} guest node{} + {} pending invite{})",
        used,
        guest_count,
        if guest_count == 1 { "" } else { "s" },
        pending,
        if pending == 1 { "" } else { "s" },
    )
}

/// `11 of 12 used`, or `11 used (no limit)` on an unlimited backend.
fn fmt_used(current: i32, limit: Option<i32>) -> String {
    match limit {
        Some(limit) => format!("{} of {} used", current, limit),
        None => format!("{} used (no limit)", current),
    }
}

/// `managed (connect.wispers.dev)`, or `self-hosted (<url>)` for a custom
/// backend.
fn backend_label(backend: Option<&str>) -> String {
    match backend {
        Some(b) => format!("self-hosted ({})", b),
        None => {
            let host = wcbe::MANAGED_API_BASE
                .trim_start_matches("https://")
                .trim_end_matches("/api/v1");
            format!("managed ({})", host)
        }
    }
}

//-- Time formatting -----------------------------------------------------------

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// `2026-05-02 14:11 UTC`, for absolute timestamps.
fn fmt_utc(rfc3339: &str) -> String {
    match parse_rfc3339(rfc3339) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => rfc3339.to_owned(),
    }
}

/// Age of a past timestamp as a compact duration, e.g. `2d 3h`.
fn fmt_age(rfc3339: &str) -> String {
    match parse_rfc3339(rfc3339) {
        Some(dt) => fmt_duration((Utc::now() - dt).num_seconds().max(0) as u64),
        None => "?".to_owned(),
    }
}

/// Time until a future timestamp as a compact duration, e.g. `23h 4m`.
fn fmt_until(rfc3339: &str) -> String {
    match parse_rfc3339(rfc3339) {
        Some(dt) => fmt_duration((dt - Utc::now()).num_seconds().max(0) as u64),
        None => "?".to_owned(),
    }
}

/// Age of a past timestamp in prose, e.g. `2 min ago`.
fn fmt_ago(rfc3339: &str) -> String {
    let Some(dt) = parse_rfc3339(rfc3339) else {
        return rfc3339.to_owned();
    };
    let secs = (Utc::now() - dt).num_seconds().max(0) as u64;
    match secs {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        86400..172800 => "1 day ago".to_owned(),
        _ => format!("{} days ago", secs / 86400),
    }
}

fn fmt_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{}d {}h", d, h)
    } else if h > 0 {
        format!("{}h {}m", h, m)
    } else if m > 0 {
        format!("{}m", m)
    } else {
        format!("{}s", secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_render_compactly() {
        assert_eq!(fmt_duration(45), "45s");
        assert_eq!(fmt_duration(150), "2m");
        assert_eq!(fmt_duration(3 * 3600 + 12 * 60), "3h 12m");
        assert_eq!(fmt_duration(2 * 86400 + 3 * 3600), "2d 3h");
    }

    // The JSON keys are a stable contract (unlike the human output).
    // This pins the camelCase naming and the null-for-unknown convention.
    #[test]
    fn json_report_shape() {
        let report = StatusReport {
            shares: vec![ShareStatus {
                name: "family".to_owned(),
                display_name: Some("Family".to_owned()),
                config_error: None,
                transport: Some(TransportStatus::WispersConnect {
                    backend: None,
                    connectivity_group_id: Some("cg-1".to_owned()),
                    group_created_at: None,
                    node_quota: Some(wcbe::NodeQuota {
                        limit: Some(12),
                        current: 11,
                    }),
                }),
                server: ServerStatus::offline(),
                apps: vec![AppStatus {
                    id: "jellyfin".to_owned(),
                    name: "Jellyfin".to_owned(),
                    kind: AppKind::Web,
                    upstream: "localhost:8096".to_owned(),
                    upstream_reachable: false,
                }],
                guests: None,
                guests_error: None,
                invites: Some(vec![InviteStatus {
                    node_name: Some("Nick's iPhone".to_owned()),
                    user_id: Some("nick@example.com".to_owned()),
                    created_at: "2026-07-20T09:00:00Z".to_owned(),
                    expires_at: "2026-07-21T09:00:00Z".to_owned(),
                    used_at: None,
                    status: "pending",
                }]),
                invites_error: None,
            }],
            groups_quota: None,
        };
        let json = serde_json::to_value(&report).unwrap();
        let share = &json["shares"][0];
        assert_eq!(share["name"], "family");
        assert_eq!(share["displayName"], "Family");
        assert_eq!(share["transport"]["kind"], "wispers-connect");
        assert_eq!(share["transport"]["backend"], serde_json::Value::Null);
        assert_eq!(share["transport"]["connectivityGroupId"], "cg-1");
        assert_eq!(share["transport"]["nodeQuota"]["limit"], 12);
        assert_eq!(share["transport"]["nodeQuota"]["current"], 11);
        assert_eq!(share["server"]["state"], "offline");
        assert_eq!(share["server"]["reachable"], serde_json::Value::Null);
        assert_eq!(share["server"]["reloadPending"], serde_json::Value::Null);
        assert_eq!(share["apps"][0]["id"], "jellyfin");
        assert_eq!(share["apps"][0]["upstream"], "localhost:8096");
        assert_eq!(share["apps"][0]["upstreamReachable"], false);
        assert_eq!(share["apps"][0]["kind"], "web");
        assert_eq!(share["guests"], serde_json::Value::Null);
        let invite = &share["invites"][0];
        assert_eq!(invite["nodeName"], "Nick's iPhone");
        assert_eq!(invite["userId"], "nick@example.com");
        assert_eq!(invite["usedAt"], serde_json::Value::Null);
        assert_eq!(invite["status"], "pending");
        // Errors are omitted, not null, when absent.
        assert!(share["server"].get("error").is_none());
        assert!(share.get("configError").is_none());
        assert!(share.get("guestsError").is_none());
        assert!(share.get("invitesError").is_none());
        // Single-share reports have no fleet-level groups quota; omitted.
        assert!(json.get("groupsQuota").is_none());
    }

    #[test]
    fn groups_quota_json_shape() {
        let report = StatusReport {
            shares: vec![],
            groups_quota: Some(vec![GroupsQuota {
                backend: None,
                shares: vec!["family".to_owned()],
                key_id: Some("wc_prod_1a2B3c4D5e6F7g8H9".to_owned()),
                count: 5,
                max: Some(7),
            }]),
        };
        let json = serde_json::to_value(&report).unwrap();
        let quota = &json["groupsQuota"][0];
        assert_eq!(quota["backend"], serde_json::Value::Null);
        assert_eq!(quota["shares"][0], "family");
        assert_eq!(quota["keyId"], "wc_prod_1a2B3c4D5e6F7g8H9");
        assert_eq!(quota["count"], 5);
        assert_eq!(quota["max"], 7);
    }

    #[test]
    fn quota_renders_pending_breakdown() {
        let quota = |limit, current| wcbe::NodeQuota { limit, current };
        assert_eq!(
            fmt_quota(&quota(Some(12), 11), 8),
            "11 of 12 used (this host + 8 guest nodes + 2 pending invites)"
        );
        // No pending invites: the guest count would just repeat the table.
        assert_eq!(fmt_quota(&quota(Some(12), 9), 8), "9 of 12 used");
        assert_eq!(
            fmt_quota(&quota(Some(12), 3), 1),
            "3 of 12 used (this host + 1 guest node + 1 pending invite)"
        );
        assert_eq!(fmt_quota(&quota(None, 3), 2), "3 used (no limit)");
    }

    #[test]
    fn live_connections_overlay_guests() {
        let guest = |node_number| GuestStatus {
            node_number,
            name: None,
            user_id: None,
            created_at: "2026-07-01T00:00:00Z".to_owned(),
            last_seen_at: None,
            connected_to_server: None,
            connected_since: None,
            revoked: false,
            revoked_at: None,
        };
        let mut guests = vec![guest(2), guest(3)];
        let connected = vec![ipc::GuestData {
            node_number: 2,
            user_id: Some("lara@example.com".to_owned()),
            connected_since: Some("2026-07-20T10:00:00Z".to_owned()),
        }];
        apply_live_connections(&mut guests, &connected);

        // A live guest gets the connection and its start time.
        assert_eq!(guests[0].connected_to_server, Some(true));
        assert_eq!(
            guests[0].connected_since.as_deref(),
            Some("2026-07-20T10:00:00Z")
        );
        // A guest without a connection is authoritatively not connected.
        assert_eq!(guests[1].connected_to_server, Some(false));
    }

    #[test]
    fn invite_status_derivation() {
        let now = parse_rfc3339("2026-07-20T12:00:00Z").unwrap();
        // Used wins, even over expiry.
        assert_eq!(
            invite_status(Some("2026-07-20T11:00:00Z"), "2026-07-19T00:00:00Z", now),
            "used"
        );
        assert_eq!(invite_status(None, "2026-07-21T12:00:00Z", now), "pending");
        assert_eq!(invite_status(None, "2026-07-20T11:59:59Z", now), "expired");
        // Unparseable expiry defaults to pending rather than crying wolf.
        assert_eq!(invite_status(None, "garbage", now), "pending");
    }
}
