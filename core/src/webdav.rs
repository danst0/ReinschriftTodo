//! WebDAV client operations for remote storage.

use anyhow::{bail, Result};
use reqwest::blocking::Client;
use serde_json;

use crate::config::{set_backend_config, BackendConfig};
use crate::i18n::t;

/// Construct a full URL from base URL and optional path.
pub fn construct_full_url(base_url: &str, path: Option<&str>) -> String {
    if let Some(p) = path {
        format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            p.trim_start_matches('/')
        )
    } else {
        base_url.to_string()
    }
}

/// Build a Nextcloud WebDAV URL from base URL and username.
pub fn nextcloud_webdav_url(base_url: &str, username: &str) -> String {
    format!(
        "{}/remote.php/dav/files/{}",
        base_url.trim_end_matches('/'),
        username
    )
}

/// Check if a URL already contains the Nextcloud WebDAV path.
pub fn is_nextcloud_url(url: &str) -> bool {
    url.contains("remote.php/dav/files")
}

/// Try a Nextcloud fallback URL if the primary URL fails.
/// Returns Ok(Some(working_base_url)) if fallback succeeds, Ok(None) if no fallback attempted.
fn try_nextcloud_fallback<F, T>(
    base_url: &str,
    path: Option<&str>,
    username: Option<&str>,
    try_request: F,
) -> Result<Option<(T, String)>>
where
    F: Fn(&str) -> Result<T>,
{
    if let Some(user) = username
        && !is_nextcloud_url(base_url) {
            let candidate_base = nextcloud_webdav_url(base_url, user);
            let candidate_full = construct_full_url(&candidate_base, path);
            if let Ok(result) = try_request(&candidate_full) {
                return Ok(Some((result, candidate_base)));
            }
        }
    Ok(None)
}

/// Get fingerprint (ETag + Last-Modified) from WebDAV server.
pub fn get_fingerprint_webdav(
    url: &str,
    path: Option<&str>,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<String> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let do_head = |target_url: &str| -> Result<String> {
        let mut req = client.head(target_url);
        if let (Some(u), Some(p)) = (username, password) {
            req = req.basic_auth(u, Some(p));
        }
        let resp = req.send()?;
        if !resp.status().is_success() {
            bail!("WebDAV error: {}", resp.status());
        }
        let etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let last_mod = resp
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        Ok(format!("{}-{}", etag, last_mod))
    };

    let full_url = construct_full_url(url, path);
    do_head(&full_url)
}

/// Read content from WebDAV server.
pub fn read_content_webdav(
    url: &str,
    path: Option<String>,
    username: Option<String>,
    password: Option<String>,
) -> Result<String> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let path_ref = path.as_deref();
    let username_ref = username.as_deref();
    let password_ref = password.as_deref();

    let try_get = |target_url: &str| -> Result<String> {
        let mut req = client.get(target_url);
        if let (Some(u), Some(p)) = (username_ref, password_ref) {
            req = req.basic_auth(u, Some(p));
        }
        let resp = req.send()?;
        if !resp.status().is_success() {
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                bail!("404 Not Found");
            }
            bail!("WebDAV error: {}", resp.status());
        }
        Ok(resp.text()?)
    };

    let full_url = construct_full_url(url, path_ref);
    match try_get(&full_url) {
        Ok(content) => Ok(content),
        Err(e) => {
            // Try Nextcloud fallback
            if let Ok(Some((content, candidate_base))) =
                try_nextcloud_fallback(url, path_ref, username_ref, try_get)
            {
                // Update internal config to use this working base URL
                set_backend_config(BackendConfig::WebDav {
                    url: candidate_base,
                    path,
                    username,
                    password,
                });
                return Ok(content);
            }
            // Return original error with hint
            if e.to_string().contains("404 Not Found") {
                bail!("WebDAV error: 404 Not Found. (Hint: For Nextcloud, ensure URL ends with /remote.php/dav/files/USERNAME)");
            }
            Err(e)
        }
    }
}

/// Write content to WebDAV server.
/// When `expected_etag` is provided, sends an `If-Match` header and returns
/// `ConflictError` on HTTP 412 Precondition Failed.
pub fn write_content_webdav(
    url: &str,
    path: Option<String>,
    username: Option<String>,
    password: Option<String>,
    content: String,
    expected_etag: Option<&str>,
) -> Result<()> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let path_ref = path.as_deref();
    let username_ref = username.as_deref();
    let password_ref = password.as_deref();
    let content_clone = content.clone();
    let etag_clone = expected_etag.map(|s| s.to_string());

    let try_put = |target_url: &str| -> Result<()> {
        let mut req = client.put(target_url);
        if let (Some(u), Some(p)) = (username_ref, password_ref) {
            req = req.basic_auth(u, Some(p));
        }
        if let Some(ref etag) = etag_clone {
            // Extract just the ETag portion (fingerprint is "etag-lastmod")
            let etag_value = etag.split('-').next().unwrap_or(etag);
            if !etag_value.is_empty() {
                req = req.header("If-Match", etag_value);
            }
        }
        req = req.body(content_clone.clone());
        let resp = req.send()?;
        if !resp.status().is_success() {
            if resp.status() == reqwest::StatusCode::PRECONDITION_FAILED {
                return Err(crate::conflict::ConflictError {
                    local_fingerprint: etag_clone.clone().unwrap_or_default(),
                    remote_fingerprint: resp
                        .headers()
                        .get("etag")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("unknown")
                        .to_string(),
                }
                .into());
            }
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                bail!("404 Not Found");
            }
            bail!("WebDAV error: {}", resp.status());
        }
        Ok(())
    };

    let full_url = construct_full_url(url, path_ref);
    match try_put(&full_url) {
        Ok(_) => Ok(()),
        Err(e) => {
            // Don't retry on conflict — propagate immediately
            if e.downcast_ref::<crate::conflict::ConflictError>().is_some() {
                return Err(e);
            }
            // Try Nextcloud fallback
            if let Ok(Some((_, candidate_base))) =
                try_nextcloud_fallback(url, path_ref, username_ref, try_put)
            {
                // Update internal config
                set_backend_config(BackendConfig::WebDav {
                    url: candidate_base,
                    path,
                    username,
                    password,
                });
                return Ok(());
            }
            if e.to_string().contains("404 Not Found") {
                bail!("WebDAV error: 404 Not Found. (Hint: For Nextcloud, ensure URL ends with /remote.php/dav/files/USERNAME)");
            }
            Err(e)
        }
    }
}

/// Initiate Nextcloud Login Flow v2.
/// Returns (login_url, poll_endpoint, poll_token).
pub fn initiate_nextcloud_login(server_url: &str) -> Result<(String, String, String)> {
    let url = format!(
        "{}/index.php/login/v2",
        server_url.trim_end_matches('/')
    );

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let resp = client.post(&url).send()?;
    if !resp.status().is_success() {
        bail!("Login Flow v2 initiation failed: HTTP {}", resp.status());
    }

    let body: serde_json::Value = resp.json()?;
    let login = body["login"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'login' field in response"))?
        .to_string();
    let endpoint = body["poll"]["endpoint"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'poll.endpoint' field in response"))?
        .to_string();
    let token = body["poll"]["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'poll.token' field in response"))?
        .to_string();

    Ok((login, endpoint, token))
}

/// Poll the Login Flow v2 endpoint.
/// Returns Ok(Some((server, login_name, app_password))) when the user has completed login,
/// Ok(None) if still pending.
pub fn poll_nextcloud_login(endpoint: &str, token: &str) -> Result<Option<(String, String, String)>> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp = client
        .post(endpoint)
        .form(&[("token", token)])
        .send()?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        bail!("Login Flow v2 poll failed: HTTP {}", resp.status());
    }

    let body: serde_json::Value = resp.json()?;
    let server = body["server"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'server' field in poll response"))?
        .to_string();
    let login_name = body["loginName"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'loginName' field in poll response"))?
        .to_string();
    let app_password = body["appPassword"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'appPassword' field in poll response"))?
        .to_string();

    Ok(Some((server, login_name, app_password)))
}

/// Test WebDAV connection without modifying config.
pub fn test_webdav_connection(
    base_url: &str,
    path: Option<&str>,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<()> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let try_connect = |target_url: &str| -> Result<()> {
        // Try HEAD first
        let mut req = client.head(target_url);
        if let (Some(u), Some(p)) = (username, password) {
            req = req.basic_auth(u, Some(p));
        }

        let resp = req.send()?;
        if resp.status().is_success() {
            return Ok(());
        }

        // Fallback to GET
        let mut req_get = client.get(target_url);
        if let (Some(u), Some(p)) = (username, password) {
            req_get = req_get.basic_auth(u, Some(p));
        }
        let resp_get = req_get.send()?;
        if !resp_get.status().is_success() {
            if resp_get.status() == reqwest::StatusCode::NOT_FOUND {
                bail!("404 Not Found");
            }
            bail!("HTTP {}", resp_get.status());
        }
        Ok(())
    };

    let full_url = construct_full_url(base_url, path);
    match try_connect(&full_url) {
        Ok(_) => Ok(()),
        Err(e) => {
            // Try Nextcloud fallback (test only, don't update config)
            if let Some(user) = username
                && !is_nextcloud_url(base_url) {
                    let candidate_base = nextcloud_webdav_url(base_url, user);
                    let candidate_full = construct_full_url(&candidate_base, path);
                    if try_connect(&candidate_full).is_ok() {
                        return Ok(());
                    }
                }
            // Return original error with context
            bail!(
                "{}",
                t("Connection to '{}' failed: {}")
                    .replace("{}", &full_url)
                    .replace("{}", &e.to_string())
            );
        }
    }
}
