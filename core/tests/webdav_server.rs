//! Integration tests against a minimal WebDAV server that mimics the parts of
//! sabre/dav (the layer Nextcloud uses) that issue #11 hinges on:
//! a PUT onto a collection, or into a missing parent, answers 409 Conflict.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use reinschrift_core::webdav::{test_webdav_connection, write_content_webdav};

const USER: &str = "alice";

#[derive(Default)]
struct Store {
    collections: HashSet<String>,
    files: HashSet<String>,
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
            files: HashSet::new(),
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
        self.store.lock().unwrap().files.contains(&path)
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
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/xml\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
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

    // Drain headers, remembering the body length.
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        let _ = reader.read_exact(&mut body);
    }

    let mut store = store.lock().unwrap();
    let is_collection = store.collections.contains(&path);
    let is_file = store.files.contains(&path);

    match method.as_str() {
        "HEAD" | "GET" => {
            if is_collection || is_file {
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
                store.files.insert(path);
                respond(&mut stream, "201 Created", "");
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
    assert!(
        !message.contains("409"),
        "the raw status code should not reach the user: {message}"
    );
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
    assert!(
        !message.contains("409"),
        "the raw status code should not reach the user: {message}"
    );
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
