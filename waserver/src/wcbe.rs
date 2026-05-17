//! Wispers Connect backend interface.
//!
//! This talks to the Wispers Connect backend to manage connectivity groups and
//! register new guest nodes.

use anyhow::{Context, Result};
use reqwest;
use serde::Deserialize;
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

impl Client {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_owned(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn add_connectivity_group(&self, share: &str) -> Result<ConnectivityGroupId> {
        let url = format!("{BASE_URL}/connectivity-groups");
        let mut body = HashMap::new();
        body.insert("name", format!("Wispers Access share '{}'", share));
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

    pub async fn get_registration_token(&self, cg_id: &str, name: &str) -> Result<String> {
        let url = format!("{BASE_URL}/connectivity-groups/{cg_id}/registration-tokens");
        let mut body = HashMap::new();
        body.insert("name", name.to_owned());
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
