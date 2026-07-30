//! Wispers Connect backend interface.
//!
//! This talks to the Wispers Connect backend to manage connectivity groups and
//! register new guest nodes.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

type ConnectivityGroupId = String;

pub const MANAGED_API_BASE: &str = "https://connect.wispers.dev/api/v1";

/// The public ID part of an API key: everything before the dot in
/// `wc_<env>_<id>.<secret>`. Safe to display. `None` when the key doesn't
/// have that shape.
pub fn key_id(api_key: &str) -> Option<&str> {
    let (id, _secret) = api_key.split_once('.')?;
    Some(id)
}

/// The Integrator REST API base to use, based on the --backend flag.
pub fn api_base(backend: Option<&str>) -> String {
    match backend {
        Some(b) => format!("{}/api/v1", b.trim_end_matches('/')),
        None => MANAGED_API_BASE.to_string(),
    }
}

#[derive(Clone)]
pub struct Client {
    api_base: String,
    api_key: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddGroupResponse {
    id: String,
    // Other fields ignored.
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationTokenResponse {
    token: String,
    // Other fields ignored.
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeMetadata {
    pub user_id: String,
}

/// A connectivity group as returned by `GET /connectivity-groups/:id`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupDetail {
    pub created_at: String, // RFC 3339
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub nodes: Vec<GroupNode>,
    /// `None` on backends that predate the field.
    #[serde(default)]
    pub node_quota: Option<NodeQuota>,
}

/// A group's node-quota usage. `current` counts registered nodes plus
/// unexpired pending registration tokens. The quota spent on pending tokens is
/// `current` minus the length of `nodes`.
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeQuota {
    /// `None` = unlimited (a backend without plans, e.g. standalone).
    pub limit: Option<i32>,
    pub current: i32,
}

/// Domain-level stats as returned by `GET /stats`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub connectivity_groups: GroupsStats,
}

/// The domain's connectivity-group quota usage.
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupsStats {
    pub count: i32,
    /// `None` = unlimited (a backend without plans, e.g. standalone).
    pub max: Option<i32>,
}

/// One entry of `GET /connectivity-groups/:id/registration-tokens`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationToken {
    #[serde(default)]
    pub node_name: Option<String>,
    #[serde(default)]
    pub node_metadata: Option<String>,
    pub created_at: String, // RFC 3339
    pub expires_at: String, // RFC 3339
    #[serde(default)]
    pub used_at: Option<String>, // RFC 3339
}

/// Outcome of `delete_node`: 404 means the registration is already gone
/// (deleted earlier, or the node deregistered itself), which callers treat
/// as "nothing left to do" rather than an error.
#[derive(Debug, PartialEq, Eq)]
pub enum DeleteNodeOutcome {
    Deleted,
    AlreadyGone,
}

/// A quota rejection (HTTP 429 with a `quota exceeded` body). Callers
/// downcast to this to render actionable messages. `Display` is the
/// fallback rendering.
#[derive(Debug, Clone)]
pub struct QuotaExceeded {
    /// Which quota, e.g. `nodes_per_group` or `groups_per_domain`.
    pub quota: String,
    pub limit: i32,
    pub current: i32,
}

impl std::fmt::Display for QuotaExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} quota exceeded ({} of {} used)",
            self.quota, self.current, self.limit
        )
    }
}

impl std::error::Error for QuotaExceeded {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupNode {
    pub node_number: i32,
    pub created_at: String, // RFC 3339
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub last_seen_at: Option<String>, // RFC 3339
    #[serde(default)]
    pub metadata: Option<String>,
}

impl Client {
    pub fn new(api_key: &str, api_base: &str) -> Self {
        Self {
            api_base: api_base.to_owned(),
            api_key: api_key.to_owned(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn get_stats(&self) -> Result<Stats> {
        let url = format!("{}/stats", self.api_base);
        let res = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to send request")?;
        let res = ok_or_error(res).await?;
        res.json().await.context("failed to parse response")
    }

    pub async fn add_connectivity_group(&self, name: &str) -> Result<ConnectivityGroupId> {
        let url = format!("{}/connectivity-groups", self.api_base);
        let mut body = HashMap::new();
        body.insert("name", name);
        let res = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to send request")?;
        let res = ok_or_error(res).await?;
        let data: AddGroupResponse = res.json().await.context("failed to parse response")?;
        Ok(data.id)
    }

    pub async fn remove_connectivity_group(&self, id: &str) -> Result<()> {
        let url = format!("{}/connectivity-groups/{id}", self.api_base);
        let res = self
            .client
            .delete(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to send request")?;
        ok_or_error(res).await?;
        Ok(())
    }

    pub async fn get_connectivity_group(&self, cg_id: &str) -> Result<GroupDetail> {
        let url = format!("{}/connectivity-groups/{cg_id}", self.api_base);
        let res = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to send request")?;
        let res = ok_or_error(res).await?;
        res.json().await.context("failed to parse response")
    }

    /// Lists recent registration tokens (pending ones plus 7-day history).
    pub async fn list_registration_tokens(&self, cg_id: &str) -> Result<Vec<RegistrationToken>> {
        let url = format!(
            "{}/connectivity-groups/{cg_id}/registration-tokens",
            self.api_base
        );
        let res = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to send request")?;
        let res = ok_or_error(res).await?;
        res.json().await.context("failed to parse response")
    }

    /// Deletes a node's registration, freeing the quota it occupies. The backend
    /// only permits this once the group's roster marks the node revoked
    /// (409 otherwise), so callers revoke first and delete second.
    pub async fn delete_node(&self, cg_id: &str, node_number: i32) -> Result<DeleteNodeOutcome> {
        let url = format!(
            "{}/connectivity-groups/{cg_id}/nodes/{node_number}",
            self.api_base
        );
        let res = self
            .client
            .delete(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to send request")?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(DeleteNodeOutcome::AlreadyGone);
        }
        ok_or_error(res).await?;
        Ok(DeleteNodeOutcome::Deleted)
    }

    pub async fn get_registration_token(
        &self,
        cg_id: &str,
        node_name: Option<&str>,
        metadata: Option<&NodeMetadata>,
    ) -> Result<String> {
        let url = format!(
            "{}/connectivity-groups/{cg_id}/registration-tokens",
            self.api_base
        );
        let mut body = HashMap::new();
        body.insert("ttlProfile", "asynchronous".to_owned());
        if let Some(node_name) = node_name {
            body.insert("nodeName", node_name.to_owned());
        }
        if let Some(metadata) = metadata {
            let json = serde_json::to_string(metadata)?;
            body.insert("nodeMetadata", json);
        }
        let res = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to send request")?;
        let res = ok_or_error(res).await?;
        let data: RegistrationTokenResponse =
            res.json().await.context("failed to parse response")?;
        Ok(data.token)
    }
}

/// Passes success responses through, everything else becomes an error —
/// quota 429s as a typed [`QuotaExceeded`], the rest as "server returned…".
async fn ok_or_error(res: reqwest::Response) -> Result<reqwest::Response> {
    if res.status().is_success() {
        return Ok(res);
    }
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    Err(classify_error(status, &body))
}

/// The backend's rate limiter also answers 429, so quota detection matches
/// on the body's `error` field, not the status alone.
fn classify_error(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    #[derive(Deserialize)]
    struct QuotaBody {
        error: String,
        quota: String,
        limit: i32,
        current: i32,
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS
        && let Ok(q) = serde_json::from_str::<QuotaBody>(body)
        && q.error == "quota exceeded"
    {
        return anyhow::Error::new(QuotaExceeded {
            quota: q.quota,
            limit: q.limit,
            current: q.current,
        });
    }
    anyhow::anyhow!("server returned {status}: {body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_detail_parses_node_quota() {
        let detail: GroupDetail = serde_json::from_str(
            r#"{"createdAt":"2026-07-30T00:00:00Z","nodes":[],"nodeQuota":{"limit":12,"current":3}}"#,
        )
        .unwrap();
        let quota = detail.node_quota.unwrap();
        assert_eq!(quota.limit, Some(12));
        assert_eq!(quota.current, 3);

        // Backends that predate the field omit it.
        let old: GroupDetail =
            serde_json::from_str(r#"{"createdAt":"2026-07-30T00:00:00Z"}"#).unwrap();
        assert!(old.node_quota.is_none());

        // `limit: null` = unlimited (standalone mode).
        let unlimited: NodeQuota = serde_json::from_str(r#"{"limit":null,"current":3}"#).unwrap();
        assert_eq!(unlimited.limit, None);
    }

    #[test]
    fn key_id_never_contains_secret_material() {
        assert_eq!(
            key_id("wc_prod_1a2B3c4D5e6F7g8H9.supersecretsupersecret"),
            Some("wc_prod_1a2B3c4D5e6F7g8H9")
        );
        // An unrecognisable key yields nothing rather than a guess.
        assert_eq!(key_id("no-dot-in-here"), None);
    }

    #[test]
    fn stats_parse_groups_quota() {
        let s: Stats =
            serde_json::from_str(r#"{"connectivityGroups":{"count":5,"max":7}}"#).unwrap();
        let groups = s.connectivity_groups;
        assert_eq!((groups.count, groups.max), (5, Some(7)));

        // `max: null` = unlimited (standalone mode).
        let s: Stats =
            serde_json::from_str(r#"{"connectivityGroups":{"count":5,"max":null}}"#).unwrap();
        assert_eq!(s.connectivity_groups.max, None);
    }

    #[test]
    fn quota_429_classifies_as_typed_error() {
        let e = classify_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":"quota exceeded","quota":"nodes_per_group","limit":12,"current":12}"#,
        );
        let q = e.downcast_ref::<QuotaExceeded>().unwrap();
        assert_eq!(
            (q.quota.as_str(), q.limit, q.current),
            ("nodes_per_group", 12, 12)
        );
        assert_eq!(
            q.to_string(),
            "nodes_per_group quota exceeded (12 of 12 used)"
        );

        // The rate limiter's 429 has a different body and stays generic.
        let e = classify_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":"rate limit exceeded","retryAfterSeconds":42}"#,
        );
        assert!(e.downcast_ref::<QuotaExceeded>().is_none());
        assert!(e.to_string().contains("429"));

        let e = classify_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "boom");
        assert!(e.to_string().contains("boom"));
    }
}
