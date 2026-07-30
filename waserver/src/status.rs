//! The `status` command.
//!
//! Overview with `waserver status`, per-share status with
//! `waserver status <share>`. Both forms gather into the same serializable
//! report, so `--json` and the human rendering never disagree.

use crate::ipc;
use crate::storage;
use crate::wcbe;
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
            let store = storage::ShareStateStore::new(share)?;
            if store.load_share_config()?.is_none() {
                anyhow::bail!("Share {} is not initialised", share);
            }
            StatusReport {
                shares: vec![gather_share(share).await],
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareStatus {
    name: String,
    display_name: Option<String>,
    /// Custom backend URL, `null` for the managed backend.
    backend: Option<String>,
    connectivity_group_id: Option<String>,
    group_created_at: Option<String>, // RFC 3339
    server: ServerStatus,
    /// `null` when the group query failed (see `membersError`).
    members: Option<Vec<Member>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    members_error: Option<String>,
    /// `null` when the query failed (see `invitesError`).
    invites: Option<Vec<Invite>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    invites_error: Option<String>,
    /// Node-quota usage of the group: `current` (members + pending invites)
    /// vs `limit` (`null` = unlimited). Omitted when the backend doesn't
    /// report it.
    #[serde(skip_serializing_if = "Option::is_none")]
    node_quota: Option<wcbe::NodeQuota>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerStatus {
    /// `serving` | `connecting` | `offline` | `error`
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    upstream: Option<String>,
    upstream_reachable: Option<bool>,
    hub_connected: Option<bool>,
    pid: Option<u32>,
    started_at: Option<String>,      // RFC 3339
    connected_since: Option<String>, // RFC 3339
    /// This server's own node number in the Members list.
    node_number: Option<i32>,
}

/// A recent invite. An invite showing `used` with no matching member means the
/// join was rolled back — issue a new invite.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Invite {
    node_name: Option<String>,
    user_id: Option<String>,
    created_at: String,      // RFC 3339
    expires_at: String,      // RFC 3339
    used_at: Option<String>, // RFC 3339
    /// `pending` | `used` | `expired`
    status: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Member {
    node_number: i32,
    name: Option<String>,
    user_id: Option<String>,
    created_at: String,           // RFC 3339
    last_seen_at: Option<String>, // RFC 3339
    /// Does the guest have a live P2P connection to this server right now?
    connected_to_server: Option<bool>,
    connected_since: Option<String>, // RFC 3339
}

//-- Gathering -----------------------------------------------------------------

async fn gather_fleet() -> Result<StatusReport> {
    let mut names = storage::list_shares()?;
    names.sort();
    // Query all shares concurrently.
    let mut tasks = tokio::task::JoinSet::new();
    for (i, name) in names.iter().enumerate() {
        let name = name.clone();
        tasks.spawn(async move { (i, gather_share(&name).await) });
    }
    let mut shares: Vec<Option<ShareStatus>> = names.iter().map(|_| None).collect();
    while let Some(joined) = tasks.join_next().await {
        let (i, share) = joined.context("status task failed")?;
        shares[i] = Some(share);
    }
    Ok(StatusReport {
        shares: shares.into_iter().flatten().collect(),
    })
}

async fn gather_share(name: &str) -> ShareStatus {
    let config = storage::ShareStateStore::new(name)
        .ok()
        .and_then(|s| s.load_share_config().ok())
        .flatten();
    let (server, group, invites) = tokio::join!(
        query_server(name),
        query_connectivity_group(config.as_ref()),
        query_invites(config.as_ref())
    );
    let (server, live_peers) = server;
    let (group, members_error) = match group {
        Ok(g) => (Some(g), None),
        Err(e) => (None, Some(e)),
    };
    let (invites, invites_error) = match invites {
        Ok(tokens) => (Some(tokens.iter().map(to_invite).collect()), None),
        Err(e) => (None, Some(e)),
    };
    let mut members = group.as_ref().map(to_members);
    match (members.as_mut(), live_peers.as_ref()) {
        (Some(members), Some(peers)) => apply_live_connections(members, peers, server.node_number),
        // A stopped server has no connections.
        (Some(members), None) if server.state == "offline" => {
            for m in members.iter_mut() {
                m.connected_to_server = Some(false);
            }
        }
        _ => {}
    }
    ShareStatus {
        name: name.to_owned(),
        display_name: group.as_ref().and_then(|g| g.name.clone()),
        backend: config.as_ref().and_then(|c| c.backend.clone()),
        connectivity_group_id: config.map(|c| c.connectivity_group_id),
        group_created_at: group.as_ref().map(|g| g.created_at.clone()),
        server,
        members,
        members_error,
        invites,
        invites_error,
        node_quota: group.as_ref().and_then(|g| g.node_quota),
    }
}

/// Overlay the server's live view onto the backend's member list: while the
/// daemon runs it knows authoritatively which guests are connected to it.
fn apply_live_connections(
    members: &mut [Member],
    peers: &[ipc::PeerData],
    own_node_number: Option<i32>,
) {
    for m in members.iter_mut() {
        if own_node_number == Some(m.node_number) {
            continue; // the server itself; "connected to server" is meaningless
        }
        match peers.iter().find(|p| p.node_number == m.node_number) {
            Some(p) => {
                m.connected_to_server = Some(true);
                m.connected_since = p.connected_since.clone();
            }
            None => m.connected_to_server = Some(false),
        }
    }
}

/// Queries the daemon; the second element is its live-peer list (`None` when
/// the daemon is down or predates peer tracking).
async fn query_server(share: &str) -> (ServerStatus, Option<Vec<ipc::PeerData>>) {
    let Ok(mut client) = ipc::Client::connect(share).await else {
        return (ServerStatus::offline(), None);
    };
    match client.request(&ipc::Request::Status).await {
        Ok(ipc::Response::Success {
            data: ipc::ResponseData::Status(s),
            ..
        }) => {
            let upstream_reachable = probe_upstream(&s.upstream).await;
            let status = ServerStatus {
                state: if s.connected_to_hub {
                    "serving"
                } else {
                    "connecting"
                },
                error: None,
                upstream: Some(s.upstream),
                upstream_reachable: Some(upstream_reachable),
                hub_connected: Some(s.connected_to_hub),
                pid: s.pid,
                started_at: s.started_at,
                connected_since: s.connected_since,
                node_number: s.node_number,
            };
            (status, s.connected_peers)
        }
        Ok(ipc::Response::Success { .. }) => {
            (ServerStatus::error("unexpected response from server"), None)
        }
        Ok(ipc::Response::Error { error, .. }) => (ServerStatus::error(error), None),
        // Probably went down just now.
        Err(_) => (ServerStatus::offline(), None),
    }
}

impl ServerStatus {
    fn offline() -> Self {
        Self {
            state: "offline",
            error: None,
            upstream: None,
            upstream_reachable: None,
            hub_connected: None,
            pid: None,
            started_at: None,
            connected_since: None,
            node_number: None,
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

async fn query_connectivity_group(
    config: Option<&storage::ShareConfig>,
) -> Result<wcbe::GroupDetail, String> {
    let Some(cfg) = config else {
        return Err("share is not initialised".to_owned());
    };
    let client = wcbe::Client::new(&cfg.api_key, &wcbe::api_base(cfg.backend.as_deref()));
    client
        .get_connectivity_group(&cfg.connectivity_group_id)
        .await
        .map_err(|e| format!("{:#}", e))
}

async fn query_invites(
    config: Option<&storage::ShareConfig>,
) -> Result<Vec<wcbe::RegistrationToken>, String> {
    let Some(cfg) = config else {
        return Err("share is not initialised".to_owned());
    };
    let client = wcbe::Client::new(&cfg.api_key, &wcbe::api_base(cfg.backend.as_deref()));
    client
        .list_registration_tokens(&cfg.connectivity_group_id)
        .await
        .map_err(|e| format!("{:#}", e))
}

fn to_invite(token: &wcbe::RegistrationToken) -> Invite {
    Invite {
        node_name: token.node_name.clone(),
        user_id: token.node_metadata.as_deref().and_then(parse_user_id),
        created_at: token.created_at.clone(),
        expires_at: token.expires_at.clone(),
        used_at: token.used_at.clone(),
        status: invite_status(token.used_at.as_deref(), &token.expires_at, Utc::now()),
    }
}

fn invite_status(used_at: Option<&str>, expires_at: &str, now: DateTime<Utc>) -> &'static str {
    if used_at.is_some() {
        return "used";
    }
    match parse_rfc3339(expires_at) {
        Some(expiry) if expiry <= now => "expired",
        _ => "pending",
    }
}

fn to_members(group: &wcbe::GroupDetail) -> Vec<Member> {
    let mut members: Vec<Member> = group
        .nodes
        .iter()
        .map(|n| Member {
            node_number: n.node_number,
            name: n.name.clone(),
            user_id: n.metadata.as_deref().and_then(parse_user_id),
            created_at: n.created_at.clone(),
            last_seen_at: n.last_seen_at.clone(),
            connected_to_server: None,
            connected_since: None,
        })
        .collect();
    members.sort_by_key(|m| m.node_number);
    members
}

fn parse_user_id(metadata: &str) -> Option<String> {
    let meta: wcbe::NodeMetadata = serde_json::from_str(metadata).ok()?;
    Some(meta.user_id).filter(|s| !s.is_empty())
}

//-- Human rendering -----------------------------------------------------------

fn print_fleet(report: &StatusReport) {
    if report.shares.is_empty() {
        println!("No app shares found");
        return;
    }
    let mut tw = TabWriter::new(std::io::stdout().lock()).padding(2);
    writeln!(&mut tw, "SHARE\tSTATE\tHUB\tUPSTREAM\tNODES").unwrap();
    for s in &report.shares {
        let hub = match s.server.hub_connected {
            Some(true) => "connected",
            Some(false) => "not connected",
            None => "-",
        };
        let upstream = s.server.upstream.as_deref().unwrap_or("-");
        let nodes = match &s.members {
            // Live server view: connected guests / total guests.
            Some(m) if m.iter().any(|m| m.connected_to_server.is_some()) => {
                let connected = m
                    .iter()
                    .filter(|m| m.connected_to_server == Some(true))
                    .count();
                let guests = m.iter().filter(|m| m.connected_to_server.is_some()).count();
                format!("{}/{} connected", connected, guests)
            }
            Some(m) => format!("{} members", m.len()),
            None => "?".to_owned(),
        };
        writeln!(
            &mut tw,
            "{}\t{}\t{}\t{}\t{}",
            s.name, s.server.state, hub, upstream, nodes
        )
        .unwrap();
    }
    tw.flush().unwrap();
}

fn print_share_details(s: &ShareStatus) {
    let mut tw = TabWriter::new(std::io::stdout().lock()).padding(2);

    writeln!(&mut tw, "Share").unwrap();
    let name = match &s.display_name {
        Some(dn) => format!("{} ({})", s.name, dn),
        None => s.name.clone(),
    };
    writeln!(&mut tw, "  Name\t{}", name).unwrap();
    let backend = match &s.backend {
        Some(b) => format!("self-hosted ({})", b),
        None => {
            let host = wcbe::MANAGED_API_BASE
                .trim_start_matches("https://")
                .trim_end_matches("/api/v1");
            format!("managed ({})", host)
        }
    };
    writeln!(&mut tw, "  Backend\t{}", backend).unwrap();
    writeln!(
        &mut tw,
        "  Connectivity group\t{}",
        s.connectivity_group_id.as_deref().unwrap_or("-")
    )
    .unwrap();
    if let Some(created) = &s.group_created_at {
        writeln!(&mut tw, "  Created\t{}", fmt_utc(created)).unwrap();
    }
    if let Some(quota) = &s.node_quota {
        let members = s.members.as_ref().map(|m| m.len()).unwrap_or(0);
        writeln!(&mut tw, "  Quota\t{}", fmt_quota(quota, members)).unwrap();
    }

    writeln!(&mut tw, "\nServer").unwrap();
    let server = &s.server;
    let mut state = server.state.to_owned();
    if let (Some(pid), Some(started)) = (server.pid, server.started_at.as_deref()) {
        state = format!("{} (pid {}, up {})", state, pid, fmt_age(started));
    }
    if let Some(e) = &server.error {
        state = format!("{}: {}", state, e);
    }
    writeln!(&mut tw, "  State\t{}", state).unwrap();
    if let Some(upstream) = &server.upstream {
        let reachable = match server.upstream_reachable {
            Some(true) => " (reachable)",
            Some(false) => " (unreachable!)",
            None => "",
        };
        writeln!(&mut tw, "  Upstream\t{}{}", upstream, reachable).unwrap();
    }
    if let Some(connected) = server.hub_connected {
        let hub = match (connected, server.connected_since.as_deref()) {
            (true, Some(since)) => format!("connected (for {})", fmt_age(since)),
            (true, None) => "connected".to_owned(),
            (false, _) => "not connected".to_owned(),
        };
        writeln!(&mut tw, "  Hub\t{}", hub).unwrap();
    }

    writeln!(&mut tw, "\nMembers").unwrap();
    match (&s.members, &s.members_error) {
        (Some(members), _) => {
            writeln!(&mut tw, "  #\tNAME\tUSER\tLAST SEEN\tSTATUS").unwrap();
            for m in members {
                let is_self = s.server.node_number == Some(m.node_number);
                let live_now = m.connected_to_server == Some(true)
                    || (is_self && s.server.hub_connected == Some(true));
                let last_seen = if live_now {
                    "now".to_owned()
                } else {
                    match m.last_seen_at.as_deref() {
                        Some(at) => fmt_ago(at),
                        None => "-".to_owned(),
                    }
                };
                let status = if is_self {
                    "(this server)".to_owned()
                } else {
                    match m.connected_to_server {
                        Some(true) => match &m.connected_since {
                            Some(since) => format!("connected ({})", fmt_age(since)),
                            None => "connected".to_owned(),
                        },
                        Some(false) => "-".to_owned(),
                        None => "?".to_owned(),
                    }
                };
                writeln!(
                    &mut tw,
                    "  {}\t{}\t{}\t{}\t{}",
                    m.node_number,
                    m.name.as_deref().unwrap_or("-"),
                    m.user_id.as_deref().unwrap_or("-"),
                    last_seen,
                    status
                )
                .unwrap();
            }
        }
        (None, Some(e)) => writeln!(&mut tw, "  (unavailable: {})", e).unwrap(),
        (None, None) => writeln!(&mut tw, "  (unavailable)").unwrap(),
    }

    writeln!(&mut tw, "\nInvites").unwrap();
    match (&s.invites, &s.invites_error) {
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

/// Renders node-quota usage, e.g. `11 of 12 used (9 members + 2 pending
/// invites)`.
fn fmt_quota(quota: &wcbe::NodeQuota, member_count: usize) -> String {
    let used = match quota.limit {
        Some(limit) => format!("{} of {} used", quota.current, limit),
        None => format!("{} used (no limit)", quota.current),
    };
    let pending = (quota.current.max(0) as usize).saturating_sub(member_count);
    if pending == 0 {
        return used;
    }
    format!(
        "{} ({} member{} + {} pending invite{})",
        used,
        member_count,
        if member_count == 1 { "" } else { "s" },
        pending,
        if pending == 1 { "" } else { "s" },
    )
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

    #[test]
    fn user_id_comes_from_node_metadata() {
        assert_eq!(
            parse_user_id(r#"{"userId": "lara@example.com"}"#),
            Some("lara@example.com".to_owned())
        );
        assert_eq!(parse_user_id(r#"{"userId": ""}"#), None);
        assert_eq!(parse_user_id("not json"), None);
    }

    // The JSON keys are a stable contract (unlike the human output).
    // This pins the camelCase naming and the null-for-unknown convention.
    #[test]
    fn json_report_shape() {
        let report = StatusReport {
            shares: vec![ShareStatus {
                name: "myapp".to_owned(),
                display_name: Some("My App".to_owned()),
                backend: None,
                connectivity_group_id: Some("cg-1".to_owned()),
                group_created_at: None,
                server: ServerStatus::offline(),
                members: None,
                members_error: None,
                invites: Some(vec![Invite {
                    node_name: Some("Nick's iPhone".to_owned()),
                    user_id: Some("nick@example.com".to_owned()),
                    created_at: "2026-07-20T09:00:00Z".to_owned(),
                    expires_at: "2026-07-21T09:00:00Z".to_owned(),
                    used_at: None,
                    status: "pending",
                }]),
                invites_error: None,
                node_quota: Some(wcbe::NodeQuota {
                    limit: Some(12),
                    current: 11,
                }),
            }],
        };
        let json = serde_json::to_value(&report).unwrap();
        let share = &json["shares"][0];
        assert_eq!(share["name"], "myapp");
        assert_eq!(share["displayName"], "My App");
        assert_eq!(share["backend"], serde_json::Value::Null);
        assert_eq!(share["connectivityGroupId"], "cg-1");
        assert_eq!(share["server"]["state"], "offline");
        assert_eq!(share["server"]["hubConnected"], serde_json::Value::Null);
        assert_eq!(share["server"]["nodeNumber"], serde_json::Value::Null);
        assert_eq!(share["members"], serde_json::Value::Null);
        let invite = &share["invites"][0];
        assert_eq!(invite["nodeName"], "Nick's iPhone");
        assert_eq!(invite["userId"], "nick@example.com");
        assert_eq!(invite["usedAt"], serde_json::Value::Null);
        assert_eq!(invite["status"], "pending");
        // Errors are omitted, not null, when absent.
        assert!(share["server"].get("error").is_none());
        assert!(share.get("membersError").is_none());
        assert!(share.get("invitesError").is_none());
        assert_eq!(share["nodeQuota"]["limit"], 12);
        assert_eq!(share["nodeQuota"]["current"], 11);
    }

    #[test]
    fn quota_renders_pending_breakdown() {
        let quota = |limit, current| wcbe::NodeQuota { limit, current };
        assert_eq!(
            fmt_quota(&quota(Some(12), 11), 9),
            "11 of 12 used (9 members + 2 pending invites)"
        );
        // No pending invites: the member count would just repeat the table.
        assert_eq!(fmt_quota(&quota(Some(12), 9), 9), "9 of 12 used");
        assert_eq!(
            fmt_quota(&quota(Some(12), 2), 1),
            "2 of 12 used (1 member + 1 pending invite)"
        );
        assert_eq!(fmt_quota(&quota(None, 3), 3), "3 used (no limit)");
    }

    #[test]
    fn live_connections_overlay_members() {
        let member = |node_number| Member {
            node_number,
            name: None,
            user_id: None,
            created_at: "2026-07-01T00:00:00Z".to_owned(),
            last_seen_at: None,
            connected_to_server: None,
            connected_since: None,
        };
        let mut members = vec![member(1), member(2), member(3)];
        let peers = vec![ipc::PeerData {
            node_number: 2,
            user_id: Some("lara@example.com".to_owned()),
            connected_since: Some("2026-07-20T10:00:00Z".to_owned()),
        }];
        apply_live_connections(&mut members, &peers, Some(1));

        // The server's own row stays untouched.
        assert_eq!(members[0].connected_to_server, None);
        // A live guest gets the connection and its start time.
        assert_eq!(members[1].connected_to_server, Some(true));
        assert_eq!(
            members[1].connected_since.as_deref(),
            Some("2026-07-20T10:00:00Z")
        );
        // A guest without a connection is authoritatively not connected.
        assert_eq!(members[2].connected_to_server, Some(false));
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
