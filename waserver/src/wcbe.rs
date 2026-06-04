//! Wispers Connect backend interface.
//!
//! This talks to the Wispers Connect backend to manage connectivity groups and
//! register new guest nodes.

use anyhow::{Context, Result};
use reqwest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

type ConnectivityGroupId = String;

const BASE_URL: &str = "https://connect.wispers.dev/api/v1";

#[derive(Clone)]
pub struct Client {
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

impl Client {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_owned(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn add_connectivity_group(&self, name: &str) -> Result<ConnectivityGroupId> {
        let url = format!("{BASE_URL}/connectivity-groups");
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
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!("server returned {status}: {body}");
        }
        let data: AddGroupResponse = res.json().await.context("failed to parse response")?;
        Ok(data.id)
    }

    pub async fn remove_connectivity_group(&self, id: &str) -> Result<()> {
        let url = format!("{BASE_URL}/connectivity-groups/{id}");
        let res = self
            .client
            .delete(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to send request")?;
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!("server returned {status}: {body}");
        }
        Ok(())
    }

    pub async fn get_registration_token(
        &self,
        cg_id: &str,
        node_name: Option<&str>,
        metadata: Option<&NodeMetadata>,
    ) -> Result<String> {
        let url = format!("{BASE_URL}/connectivity-groups/{cg_id}/registration-tokens");
        let mut body = HashMap::new();
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
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!("server returned {status}: {body}");
        }
        let data: RegistrationTokenResponse =
            res.json().await.context("failed to parse response")?;
        Ok(data.token)
    }
}
