//! Integration tests against a minimal WebDAV server that mimics the parts of
//! sabre/dav (the layer Nextcloud uses) that issue #11 hinges on:
//! a PUT onto a collection, or into a missing parent, answers 409 Conflict.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use reinschrift_core::conflict::ConflictError;
use reinschrift_core::webdav::{
    read_content_with_fingerprint_webdav, test_webdav_connection, write_content_webdav,
};

const USER: &str = "alice";

#[derive(Default)]
struct Store {
    collections: HashSet<String>,
    /// path -> (content, version). The version stands in for Nextcloud's ETag,
    /// which changes on every write.
    files: HashMap<String, (String, u32)>,
}

struct MockServer {
    base_url: String,
    root: String,
    store: Arc<Mutex<Store>>,
}

impl MockServer {
    /// Start a server whose WebDAV root is `/remote.php/dav/files/alice`,
    /// pre-populated with the given collections.
    fn start(extra_collections: &[&str]) -> Self {
        Self::start_at(&format!("/remote.php/dav/files/{USER}"), extra_collections)
    }

    /// Start a server on a plain WebDAV root, i.e. one the Nextcloud-shaped URL
    /// heuristic cannot recognise as a folder on its own.
    fn start_at(root: &str, extra_collections: &[&str]) -> Self {
        let root = root.to_string();
        let mut collections = HashSet::from([root.clone()]);
        for c in extra_collections {
            collections.insert(format!("{root}/{c}"));
        }
        let store = Arc::new(Mutex::new(Store {
            collections,
            files: HashMap::new(),
        }));

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let port = listener.local_addr().unwrap().port();
        let store_bg = Arc::clone(&store);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let store = Arc::clone(&store_bg);
                std::thread::spawn(move || handle(stream, store));
            }
        });

        Self {
            base_url: format!("http://127.0.0.1:{port}{root}"),
            root,
            store,
        }
    }

    fn has_file(&self, relative: &str) -> bool {
        let path = format!("{}/{relative}", self.root);
        self.store.lock().unwrap().files.contains_key(&path)
    }

    /// Seed a file, as if another client had stored it.
    fn put_file(&self, relative: &str, content: &str) {
        let path = format!("{}/{relative}", self.root);
        let mut store = self.store.lock().unwrap();
        let version = store.files.get(&path).map(|(_, v)| v + 1).unwrap_or(1);
        store.files.insert(path, (content.to_string(), version));
    }

    fn content_of(&self, relative: &str) -> Option<String> {
        let path = format!("{}/{relative}", self.root);
        let store = self.store.lock().unwrap();
        store.files.get(&path).map(|(c, _)| c.clone())
    }

    fn has_collection(&self, relative: &str) -> bool {
        let path = format!("{}/{relative}", self.root);
        self.store.lock().unwrap().collections.contains(&path)
    }
}

fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => path[..i].to_string(),
    }
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    respond_with(stream, status, body, "");
}

/// Like `respond`, plus extra headers (already CRLF-terminated).
fn respond_with(stream: &mut TcpStream, status: &str, body: &str, extra_headers: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/xml\r\n{extra_headers}Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

fn etag_of(version: u32) -> String {
    format!("\"v{version}\"")
}

fn handle(mut stream: TcpStream, store: Arc<Mutex<Store>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.trim().is_empty() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    // Drain headers, remembering the body length and the If-Match validator.
    let mut content_length = 0usize;
    let mut if_match: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("if-match:") {
            if_match = Some(line[("if-match:".len())..].trim().to_string());
        }
    }
    let mut body = Vec::new();
    if content_length > 0 {
        body = vec![0u8; content_length];
        let _ = reader.read_exact(&mut body);
    }
    let body = String::from_utf8_lossy(&body).to_string();

    let mut store = store.lock().unwrap();
    let is_collection = store.collections.contains(&path);
    let is_file = store.files.contains_key(&path);

    match method.as_str() {
        "HEAD" | "GET" => {
            if let Some((content, version)) = store.files.get(&path) {
                let headers = format!(
                    "ETag: {}\r\nLast-Modified: Sat, 22 Aug 2026 15:41:20 GMT\r\n",
                    etag_of(*version)
                );
                respond_with(&mut stream, "200 OK", content, &headers);
            } else if is_collection {
                respond(&mut stream, "200 OK", "ok");
            } else {
                respond(&mut stream, "404 Not Found", "");
            }
        }
        "PROPFIND" => {
            if !is_collection && !is_file {
                respond(&mut stream, "404 Not Found", "");
                return;
            }
            let resourcetype = if is_collection {
                "<d:resourcetype><d:collection/></d:resourcetype>"
            } else {
                "<d:resourcetype/>"
            };
            let body = format!(
                r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:"><d:response><d:href>{path}</d:href><d:propstat><d:prop>{resourcetype}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#
            );
            respond(&mut stream, "207 Multi-Status", &body);
        }
        "PUT" => {
            // sabre/dav: "PUT is not allowed on non-files", and a PUT without an
            // existing parent collection must fail with 409 as well.
            if is_collection {
                respond(&mut stream, "409 Conflict", "PUT is not allowed on non-files.");
            } else if !store.collections.contains(&parent_of(&path)) {
                respond(&mut stream, "409 Conflict", "Parent node does not exist.");
            } else {
                let current = store.files.get(&path).map(|(_, v)| *v);
                let matches = match (&if_match, current) {
                    (None, _) => true,
                    (Some(tag), Some(version)) => tag == &etag_of(version),
                    (Some(_), None) => false,
                };
                if !matches {
                    let headers = current
                        .map(|v| format!("ETag: {}\r\n", etag_of(v)))
                        .unwrap_or_default();
                    respond_with(&mut stream, "412 Precondition Failed", "", &headers);
                } else {
                    let version = current.map(|v| v + 1).unwrap_or(1);
                    store.files.insert(path, (body, version));
                    respond(&mut stream, "201 Created", "");
                }
            }
        }
        "MKCOL" => {
            if is_collection || is_file {
                respond(&mut stream, "405 Method Not Allowed", "");
            } else if !store.collections.contains(&parent_of(&path)) {
                respond(&mut stream, "409 Conflict", "");
            } else {
                store.collections.insert(path);
                respond(&mut stream, "201 Created", "");
            }
        }
        _ => respond(&mut stream, "501 Not Implemented", ""),
    }
}

fn creds() -> (Option<String>, Option<String>) {
    (Some(USER.to_string()), Some("app-password".to_string()))
}

/// The hint is translated depending on the environment, so assert on the parts
/// that survive translation: the offending URL and the example file name.
fn assert_points_at_the_path_setting(message: &str, expected_url: &str) {
    assert!(
        message.contains(expected_url),
        "expected the offending URL in: {message}"
    );
    assert!(
        message.contains("todos.md"),
        "expected the example file name in: {message}"
    );
    assert_no_raw_status_code(message, expected_url);
}

/// The message must not leak a bare status code — but it does carry the URL,
/// and the mock server listens on an ephemeral port. A port like 40915 or
/// 34090 contains "409" and used to fail this assertion a few runs in a
/// hundred, with nothing wrong in the code under test. So look everywhere
/// except in the URL itself.
fn assert_no_raw_status_code(message: &str, url: &str) {
    let outside_the_url = message.replace(url, "");
    assert!(
        !outside_the_url.contains("409"),
        "the raw status code should not reach the user: {message}"
    );
}

/// Pins the flake itself: the check must survive a port that reads like a
/// status code, and must still catch a status code that really did leak.
#[test]
fn a_port_that_looks_like_a_status_code_is_not_mistaken_for_one() {
    let url = "http://127.0.0.1:40915/remote.php/dav/files/alice";

    assert_no_raw_status_code(
        &format!("'{url}' is a folder, not a file. Set 'Path (relative)' to todos.md."),
        url,
    );

    let leaked = std::panic::catch_unwind(|| {
        assert_no_raw_status_code(&format!("WebDAV error: 409 Conflict at '{url}'"), url);
    });
    assert!(leaked.is_err(), "a leaked status code must still be caught");
}

/// The exact configuration from issue #11: WebDAV root as URL, no relative path.
/// The old code sent a PUT and surfaced a bare "409 Conflict".
#[test]
fn writing_without_a_path_explains_the_problem() {
    let server = MockServer::start(&[]);
    let (user, pass) = creds();

    let err = write_content_webdav(
        &server.base_url,
        None,
        user,
        pass,
        "- [ ] Test\n".to_string(),
        None,
    )
    .expect_err("writing onto the account root must fail");

    assert_points_at_the_path_setting(&err.to_string(), &server.base_url);
}

/// An empty path field must behave exactly like an unset one.
#[test]
fn an_empty_path_is_treated_as_unset() {
    let server = MockServer::start(&[]);
    let (user, pass) = creds();

    let err = write_content_webdav(
        &server.base_url,
        Some("   ".to_string()),
        user,
        pass,
        "- [ ] Test\n".to_string(),
        None,
    )
    .expect_err("a blank path must not silently target the root");

    assert_points_at_the_path_setting(&err.to_string(), &server.base_url);
}

#[test]
fn writing_to_a_file_in_the_root_works() {
    let server = MockServer::start(&[]);
    let (user, pass) = creds();

    write_content_webdav(
        &server.base_url,
        Some("todos.md".to_string()),
        user,
        pass,
        "- [ ] Test\n".to_string(),
        None,
    )
    .expect("writing a file into the account root should work");

    assert!(server.has_file("todos.md"));
}

/// The optional half of the fix: a missing parent folder is created instead of
/// bubbling up as a 409.
#[test]
fn missing_parent_folders_are_created() {
    let server = MockServer::start(&[]);
    let (user, pass) = creds();
    assert!(!server.has_collection("Notes"));

    write_content_webdav(
        &server.base_url,
        Some("Notes/Personal/todos.md".to_string()),
        user,
        pass,
        "- [ ] Test\n".to_string(),
        None,
    )
    .expect("missing parents should be created on the fly");

    assert!(server.has_collection("Notes"));
    assert!(server.has_collection("Notes/Personal"));
    assert!(server.has_file("Notes/Personal/todos.md"));
}

/// The connection test used to report success for the account root, which is
/// what made the later 409 so confusing.
#[test]
fn connection_test_rejects_the_account_root() {
    let server = MockServer::start(&[]);
    let (user, pass) = creds();

    let err = test_webdav_connection(&server.base_url, None, user.as_deref(), pass.as_deref())
        .expect_err("a reachable folder is not a usable configuration");
    assert_points_at_the_path_setting(&err.to_string(), &server.base_url);
}

/// A folder deeper in the tree is not covered by the URL heuristic — this is the
/// case PROPFIND has to catch.
#[test]
fn connection_test_detects_a_folder_via_propfind() {
    let server = MockServer::start(&["Notes"]);
    let (user, pass) = creds();

    let err = test_webdav_connection(
        &server.base_url,
        Some("Notes"),
        user.as_deref(),
        pass.as_deref(),
    )
    .expect_err("pointing at a subfolder must be rejected too");
    assert_points_at_the_path_setting(&err.to_string(), &format!("{}/Notes", server.base_url));
}

#[test]
fn connection_test_accepts_an_existing_file() {
    let server = MockServer::start(&[]);
    let (user, pass) = creds();

    write_content_webdav(
        &server.base_url,
        Some("todos.md".to_string()),
        user.clone(),
        pass.clone(),
        "- [ ] Test\n".to_string(),
        None,
    )
    .expect("seed the file");

    test_webdav_connection(
        &server.base_url,
        Some("todos.md"),
        user.as_deref(),
        pass.as_deref(),
    )
    .expect("an existing file is a valid target");
}

/// On a plain WebDAV root the URL alone gives nothing away, so the missing path
/// only surfaces once the server answers the PUT with 409.
#[test]
fn a_plain_webdav_root_without_a_path_is_explained_too() {
    let server = MockServer::start_at("/dav", &[]);
    let (user, pass) = creds();

    let err = write_content_webdav(
        &server.base_url,
        None,
        user,
        pass,
        "- [ ] Test\n".to_string(),
        None,
    )
    .expect_err("writing onto a folder must fail");

    let message = err.to_string();
    assert!(
        message.contains("todos.md"),
        "expected the example file name in: {message}"
    );
    assert_no_raw_status_code(&message, &server.base_url);
}

/// An unrelated failure must keep its own error — the path hint would only
/// send the user chasing the wrong setting.
#[test]
fn unrelated_errors_are_not_relabelled_as_a_path_problem() {
    let (user, pass) = creds();

    // Port 1 refuses the connection, so this fails before any WebDAV semantics
    // come into play. The target file is deliberately not named todos.md, so the
    // example file name in the hint cannot leak in through the URL.
    let err = write_content_webdav(
        "http://127.0.0.1:1/dav/tasks.md",
        Some(String::new()),
        user,
        pass,
        "- [ ] Test\n".to_string(),
        None,
    )
    .expect_err("a refused connection must surface as such");

    assert!(
        !err.to_string().contains("todos.md"),
        "transport errors must not be relabelled as a path problem: {err}"
    );
}

/// The validator must come from the very response that carried the bytes.
/// Fetching it separately pairs stale content with a fresh ETag — the If-Match
/// on the way out then passes and the other writer's change is overwritten.
#[test]
fn a_read_returns_the_validator_of_the_content_it_returned() {
    let server = MockServer::start(&[]);
    let (user, pass) = creds();
    server.put_file("todos.md", "- [ ] Eins ^aaa1\n");

    let (content, fingerprint) = read_content_with_fingerprint_webdav(
        &server.base_url,
        Some("todos.md".to_string()),
        user,
        pass,
    )
    .expect("read");

    assert_eq!(content, "- [ ] Eins ^aaa1\n");
    assert!(
        fingerprint.starts_with("\"v1\"-"),
        "expected the ETag of this very response, got: {fingerprint}"
    );
}

#[test]
fn a_write_against_the_current_validator_succeeds() {
    let server = MockServer::start(&[]);
    let (user, pass) = creds();
    server.put_file("todos.md", "- [ ] Eins ^aaa1\n");

    let (_, fingerprint) = read_content_with_fingerprint_webdav(
        &server.base_url,
        Some("todos.md".to_string()),
        user.clone(),
        pass.clone(),
    )
    .expect("read");

    write_content_webdav(
        &server.base_url,
        Some("todos.md".to_string()),
        user,
        pass,
        "- [x] Eins ✅ 2026-08-22 ^aaa1\n".to_string(),
        Some(&fingerprint),
    )
    .expect("write");

    assert_eq!(
        server.content_of("todos.md").as_deref(),
        Some("- [x] Eins ✅ 2026-08-22 ^aaa1\n")
    );
}

/// The reported failure: a completion written on top of a newer state. The
/// server must reject it, and the rejection must carry the change so the UI can
/// offer to force it through instead of restoring an old snapshot.
#[test]
fn a_stale_write_is_rejected_and_keeps_the_foreign_change() {
    let server = MockServer::start(&[]);
    let (user, pass) = creds();
    server.put_file("todos.md", "- [ ] Eins ^aaa1\n- [ ] Zwei ^bbb2\n");

    let (_, stale) = read_content_with_fingerprint_webdav(
        &server.base_url,
        Some("todos.md".to_string()),
        user.clone(),
        pass.clone(),
    )
    .expect("read");

    // Another client stores a completion in the meantime.
    let foreign = "- [ ] Eins ^aaa1\n- [x] Zwei ✅ 2026-08-22 ^bbb2\n";
    server.put_file("todos.md", foreign);

    let pending = "- [x] Eins ✅ 2026-08-22 ^aaa1\n- [ ] Zwei ^bbb2\n".to_string();
    let err = write_content_webdav(
        &server.base_url,
        Some("todos.md".to_string()),
        user,
        pass,
        pending.clone(),
        Some(&stale),
    )
    .expect_err("a stale write must be refused");

    let conflict = err
        .downcast_ref::<ConflictError>()
        .expect("a conflict, not a generic error");
    assert_eq!(
        conflict.pending_content.as_deref(),
        Some(pending.as_str()),
        "\"Overwrite\" needs the change it is supposed to force through"
    );
    assert_eq!(
        server.content_of("todos.md").as_deref(),
        Some(foreign),
        "the other writer's completion must survive"
    );
}
