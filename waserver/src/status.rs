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
    /// TODO: Right now this is always null. Implement when backend support exists.
    invites: serde_json::Value,
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Member {
    node_number: i32,
    name: Option<String>,
    user_id: Option<String>,
    created_at: String,           // RFC 3339
    last_seen_at: Option<String>, // RFC 3339
    is_online: Option<bool>,
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
    let (server, group) = tokio::join!(
        query_server(name),
        query_connectivity_group(config.as_ref())
    );
    let (group, members_error) = match group {
        Ok(g) => (Some(g), None),
        Err(e) => (None, Some(e)),
    };
    ShareStatus {
        name: name.to_owned(),
        display_name: group.as_ref().and_then(|g| g.name.clone()),
        backend: config.as_ref().and_then(|c| c.backend.clone()),
        connectivity_group_id: config.map(|c| c.connectivity_group_id),
        group_created_at: group.as_ref().map(|g| g.created_at.clone()),
        server,
        members: group.as_ref().map(to_members),
        members_error,
        invites: serde_json::Value::Null,
    }
}

async fn query_server(share: &str) -> ServerStatus {
    let Ok(mut client) = ipc::Client::connect(share).await else {
        return ServerStatus::offline();
    };
    match client.request(&ipc::Request::Status).await {
        Ok(ipc::Response::Success {
            data: ipc::ResponseData::Status(s),
            ..
        }) => {
            let upstream_reachable = probe_upstream(&s.upstream).await;
            ServerStatus {
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
            }
        }
        Ok(ipc::Response::Success { .. }) => ServerStatus::error("unexpected response from server"),
        Ok(ipc::Response::Error { error, .. }) => ServerStatus::error(error),
        // Probably went down just now.
        Err(_) => ServerStatus::offline(),
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
            is_online: n.is_online,
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
            Some(m) => {
                let online = m.iter().filter(|m| m.is_online == Some(true)).count();
                format!("{}/{} online", online, m.len())
            }
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
                let last_seen = match (m.is_online, m.last_seen_at.as_deref()) {
                    (Some(true), _) => "now".to_owned(),
                    (_, Some(at)) => fmt_ago(at),
                    (_, None) => "-".to_owned(),
                };
                let status = match m.is_online {
                    Some(true) => "online",
                    Some(false) => "offline",
                    None => "?",
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

    tw.flush().unwrap();
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
                invites: serde_json::Value::Null,
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
        assert_eq!(share["members"], serde_json::Value::Null);
        assert_eq!(share["invites"], serde_json::Value::Null);
        // Errors are omitted, not null, when absent.
        assert!(share["server"].get("error").is_none());
        assert!(share.get("membersError").is_none());
    }
}
