//! WebDAV client operations for remote storage.

use anyhow::{bail, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::blocking::Client;
use serde_json;

use crate::config::{set_backend_config, BackendConfig};
use crate::i18n::t;

/// PROPFIND body asking only for the resource type of the target itself.
const PROPFIND_RESOURCETYPE: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#;

/// Matches a `<collection/>` element in a PROPFIND response, whatever namespace
/// prefix the server happens to use.
static COLLECTION_RESOURCETYPE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)<[a-z0-9]*:?collection\s*/?>").unwrap());

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

/// Normalize an optional relative path: trim it, and treat an empty string as unset.
pub fn normalize_webdav_path(path: Option<String>) -> Option<String> {
    path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty())
}

/// Heuristic check whether a URL addresses a collection (folder) instead of a file.
///
/// A trailing slash is the WebDAV convention for collections, and a bare Nextcloud
/// dav root (`…/remote.php/dav/files/USER`) is always the user's home folder — which
/// is exactly what an empty "Path (relative)" resolves to.
pub fn looks_like_collection(full_url: &str) -> bool {
    let target = full_url.split(['?', '#']).next().unwrap_or(full_url);
    if target.ends_with('/') {
        return true;
    }
    if let Some(rest) = target.split("remote.php/dav/files").nth(1) {
        let rest = rest.trim_start_matches('/').trim_end_matches('/');
        // "" is the dav root itself, "USER" the user's home collection
        return !rest.contains('/');
    }
    false
}

/// Ask the server via PROPFIND (Depth: 0) whether a target is a collection.
/// Returns `None` when the server does not answer the PROPFIND at all, so that
/// servers without PROPFIND support do not turn into a false alarm.
fn probe_is_collection(
    client: &Client,
    target_url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Option<bool> {
    let method = reqwest::Method::from_bytes(b"PROPFIND").ok()?;
    let mut req = client
        .request(method, target_url)
        .header("Depth", "0")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(PROPFIND_RESOURCETYPE);
    if let (Some(u), Some(p)) = (username, password) {
        req = req.basic_auth(u, Some(p));
    }
    let resp = req.send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    Some(COLLECTION_RESOURCETYPE.is_match(&resp.text().ok()?))
}

/// Error for a target that is a folder rather than the todo file.
fn folder_target_error(full_url: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{}",
        t("'{}' is a folder, not a file. Set 'Path (relative)' to the todo file, for example todos.md.")
            .replacen("{}", full_url, 1)
    )
}

/// Error for a WebDAV backend without a configured relative path.
fn missing_path_error() -> anyhow::Error {
    anyhow::anyhow!(
        "{}",
        t("No path configured. Set 'Path (relative)' to the todo file, for example todos.md.")
    )
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
            // A folder is never a usable todo file — don't adopt it as the base URL.
            if looks_like_collection(&candidate_full) {
                return Ok(None);
            }
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
        Ok(fingerprint_from_headers(resp.headers()))
    };

    let full_url = construct_full_url(url, path);
    do_head(&full_url)
}

/// Build the fingerprint string from a response's validators.
/// Kept in one place so `HEAD` and `GET` cannot drift apart.
fn fingerprint_from_headers(headers: &reqwest::header::HeaderMap) -> String {
    let etag = headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let last_mod = headers
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    format!("{}-{}", etag, last_mod)
}

/// Read content from WebDAV server.
pub fn read_content_webdav(
    url: &str,
    path: Option<String>,
    username: Option<String>,
    password: Option<String>,
) -> Result<String> {
    read_content_with_fingerprint_webdav(url, path, username, password).map(|(content, _)| content)
}

/// Read content and its fingerprint from a *single* `GET`.
///
/// Fetching the validator separately (`GET` then `HEAD`) is unsafe: a write
/// landing between the two requests pairs old content with the new ETag, so the
/// `If-Match` guard passes while the edit is applied to a stale file — silently
/// discarding the other writer's changes. The `ETag`/`Last-Modified` of the very
/// response that carried the bytes cannot drift like that.
pub fn read_content_with_fingerprint_webdav(
    url: &str,
    path: Option<String>,
    username: Option<String>,
    password: Option<String>,
) -> Result<(String, String)> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let path = normalize_webdav_path(path);
    let path_ref = path.as_deref();
    let username_ref = username.as_deref();
    let password_ref = password.as_deref();

    let try_get = |target_url: &str| -> Result<(String, String)> {
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
        let fingerprint = fingerprint_from_headers(resp.headers());
        Ok((resp.text()?, fingerprint))
    };

    let full_url = construct_full_url(url, path_ref);
    if looks_like_collection(&full_url) {
        return Err(folder_target_error(&full_url));
    }
    match try_get(&full_url) {
        Ok(result) => Ok(result),
        Err(e) => {
            // Try Nextcloud fallback
            if let Ok(Some((result, candidate_base))) =
                try_nextcloud_fallback(url, path_ref, username_ref, try_get)
            {
                // Update internal config to use this working base URL
                set_backend_config(BackendConfig::WebDav {
                    url: candidate_base,
                    path,
                    username,
                    password,
                });
                return Ok(result);
            }
            // Return original error with hint
            if e.to_string().contains("404 Not Found") {
                bail!("WebDAV error: 404 Not Found. (Hint: For Nextcloud, ensure URL ends with /remote.php/dav/files/USERNAME)");
            }
            Err(e)
        }
    }
}

/// Create the parent collections for `path` below `base_url`, top-down.
/// Returns true when the server accepted at least one MKCOL, i.e. when a retry
/// of the PUT has a chance of succeeding.
fn ensure_parent_collections(
    client: &Client,
    base_url: &str,
    path: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> bool {
    let segments: Vec<&str> = path
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .collect();
    if segments.len() < 2 {
        // The file sits directly in the base collection — nothing to create.
        return false;
    }
    let Ok(method) = reqwest::Method::from_bytes(b"MKCOL") else {
        return false;
    };

    let mut created = false;
    let mut current = base_url.trim_end_matches('/').to_string();
    for segment in &segments[..segments.len() - 1] {
        current = format!("{}/{}", current, segment);
        let mut req = client.request(method.clone(), &current);
        if let (Some(u), Some(p)) = (username, password) {
            req = req.basic_auth(u, Some(p));
        }
        match req.send() {
            // 201 Created — the collection is new.
            Ok(resp) if resp.status().is_success() => created = true,
            // 405 Method Not Allowed — it was already there, keep walking down.
            Ok(_) => {}
            Err(_) => return created,
        }
    }
    created
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

    let path = normalize_webdav_path(path);
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
                return Err(crate::conflict::ConflictError::new(
                    etag_clone.clone().unwrap_or_default(),
                    resp.headers()
                        .get("etag")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("unknown")
                        .to_string(),
                )
                .with_pending(content_clone.clone())
                .into());
            }
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                bail!("404 Not Found");
            }
            if resp.status() == reqwest::StatusCode::CONFLICT {
                bail!("409 Conflict");
            }
            bail!("WebDAV error: {}", resp.status());
        }
        Ok(())
    };

    let full_url = construct_full_url(url, path_ref);
    if looks_like_collection(&full_url) {
        return Err(folder_target_error(&full_url));
    }
    match try_put(&full_url) {
        Ok(_) => Ok(()),
        Err(e) => {
            // Don't retry on conflict — propagate immediately
            if e.downcast_ref::<crate::conflict::ConflictError>().is_some() {
                return Err(e);
            }
            // A 409 on an otherwise reachable server usually means the parent
            // folder does not exist yet — create it and retry once.
            if e.to_string().contains("409 Conflict")
                && let Some(p) = path_ref
                && ensure_parent_collections(&client, url, p, username_ref, password_ref)
            {
                match try_put(&full_url) {
                    Ok(()) => return Ok(()),
                    // A precondition failure is a real conflict, not a missing folder.
                    Err(retry_err)
                        if retry_err
                            .downcast_ref::<crate::conflict::ConflictError>()
                            .is_some() =>
                    {
                        return Err(retry_err);
                    }
                    Err(_) => {}
                }
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
            if e.to_string().contains("409 Conflict") {
                if path.is_none() {
                    return Err(missing_path_error());
                }
                bail!(
                    "{}",
                    t("WebDAV error: 409 Conflict. The target is a folder, or its parent folder does not exist. Check 'Path (relative)' — for example todos.md.")
                );
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

    let path = path.map(str::trim).filter(|p| !p.is_empty());
    let full_url = construct_full_url(base_url, path);

    // Which URL actually answered — the primary one or the Nextcloud fallback.
    let reachable_url = match try_connect(&full_url) {
        Ok(_) => full_url.clone(),
        Err(e) => {
            // Try Nextcloud fallback (test only, don't update config)
            let mut fallback = None;
            if let Some(user) = username
                && !is_nextcloud_url(base_url) {
                    let candidate_base = nextcloud_webdav_url(base_url, user);
                    let candidate_full = construct_full_url(&candidate_base, path);
                    if try_connect(&candidate_full).is_ok() {
                        fallback = Some(candidate_full);
                    }
                }
            match fallback {
                Some(url) => url,
                // Return original error with context
                None => bail!(
                    "{}",
                    t("Connection to '{}' failed: {}")
                        .replacen("{}", &full_url, 1)
                        .replacen("{}", &e.to_string(), 1)
                ),
            }
        }
    };

    // Reachable is not enough: a folder answers happily but can never hold the
    // todos, and the PUT would only fail later with a bare 409.
    if looks_like_collection(&reachable_url)
        || probe_is_collection(&client, &reachable_url, username, password) == Some(true)
    {
        return Err(folder_target_error(&reachable_url));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_path_counts_as_unset() {
        assert_eq!(normalize_webdav_path(None), None);
        assert_eq!(normalize_webdav_path(Some(String::new())), None);
        assert_eq!(normalize_webdav_path(Some("   ".to_string())), None);
        assert_eq!(
            normalize_webdav_path(Some("  todos.md  ".to_string())),
            Some("todos.md".to_string())
        );
    }

    #[test]
    fn nextcloud_root_is_recognised_as_folder() {
        // The exact configuration from issue #11: login flow filled in the URL,
        // "Path (relative)" stayed empty.
        let base = "https://cloud.example.com/remote.php/dav/files/alice";
        assert!(looks_like_collection(&construct_full_url(base, None)));
        assert!(looks_like_collection(&construct_full_url(base, Some(""))));
        assert!(looks_like_collection(
            "https://cloud.example.com/remote.php/dav/files"
        ));
    }

    #[test]
    fn file_targets_are_not_folders() {
        let base = "https://cloud.example.com/remote.php/dav/files/alice";
        assert!(!looks_like_collection(&construct_full_url(
            base,
            Some("todos.md")
        )));
        assert!(!looks_like_collection(&construct_full_url(
            base,
            Some("Notes/todos.md")
        )));
        assert!(!looks_like_collection("https://dav.example.com/todos.md"));
    }

    #[test]
    fn trailing_slash_marks_a_collection() {
        assert!(looks_like_collection("https://dav.example.com/notes/"));
        assert!(!looks_like_collection("https://dav.example.com/notes"));
    }

    #[test]
    fn collection_resourcetype_is_detected_regardless_of_prefix() {
        assert!(COLLECTION_RESOURCETYPE.is_match("<d:resourcetype><d:collection/></d:resourcetype>"));
        assert!(COLLECTION_RESOURCETYPE.is_match("<D:resourcetype><D:collection /></D:resourcetype>"));
        assert!(COLLECTION_RESOURCETYPE.is_match("<resourcetype><collection/></resourcetype>"));
        assert!(!COLLECTION_RESOURCETYPE.is_match("<d:resourcetype/>"));
    }
}
