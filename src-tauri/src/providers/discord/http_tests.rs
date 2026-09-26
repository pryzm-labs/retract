use super::http::{
    DeleteOutcome, DiscordDeleteClient, DiscordDeleteError, DiscordLiveMessageBinding,
};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Duration;

const TOKEN: &str = "synthetic.discord.token-value_123456789";
const CONTENT: &str = "synthetic message";
const TIMESTAMP_MILLIS: i64 = 1_893_553_445_123;

fn binding() -> DiscordLiveMessageBinding {
    DiscordLiveMessageBinding::new(CONTENT, TIMESTAMP_MILLIS)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn server(
    status: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    let headers = headers
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    let body = body.to_owned();
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = vec![0_u8; 16 * 1024];
        let count = socket.read(&mut request).unwrap();
        request.truncate(count);
        let request = String::from_utf8(request).unwrap();
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            response.push_str(&format!("{name}: {value}\r\n"));
        }
        response.push_str("\r\n");
        response.push_str(&body);
        socket.write_all(response.as_bytes()).unwrap();
        request
    });
    (format!("http://{address}/api/v10/"), handle)
}

fn scripted_server(responses: Vec<(String, String)>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        responses
            .into_iter()
            .map(|(status, body)| {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = vec![0_u8; 16 * 1024];
                let count = socket.read(&mut request).unwrap();
                request.truncate(count);
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).unwrap();
                String::from_utf8(request).unwrap()
            })
            .collect()
    });
    (format!("http://{address}/api/v10/"), handle)
}

fn live_message() -> String {
    r#"{"id":"1985931830091579393","channel_id":"9007199254741101","author":{"id":"9007199254741001"},"content":"synthetic message","timestamp":"2030-01-02T03:04:05.123000+00:00"}"#.into()
}

fn run_verify(
    status: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (Result<(), DiscordDeleteError>, String) {
    let (base, handle) = server(status, headers, body);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let client = DiscordDeleteClient::for_test(&base).unwrap();
    let result = runtime.block_on(client.verify_owned(
        "9007199254741101",
        "1985931830091579393",
        "9007199254741001",
        &binding(),
        TOKEN,
        &AtomicBool::new(false),
    ));
    (result, handle.join().unwrap())
}

fn stalled_rate_limit_server(
    response_head: String,
    body_prefix: Vec<u8>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = vec![0_u8; 16 * 1024];
        let _ = socket.read(&mut request).unwrap();
        socket.write_all(response_head.as_bytes()).unwrap();
        socket.write_all(&body_prefix).unwrap();
        socket.flush().unwrap();
        thread::sleep(Duration::from_secs(1));
    });
    (format!("http://{address}/api/v10/"), handle)
}

fn run_verify_with_timeout(
    base: &str,
) -> Result<Result<(), DiscordDeleteError>, tokio::time::error::Elapsed> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let client = DiscordDeleteClient::for_test(base).unwrap();
    runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_millis(400),
            client.verify_owned(
                "9007199254741101",
                "1985931830091579393",
                "9007199254741001",
                &binding(),
                TOKEN,
                &AtomicBool::new(false),
            ),
        )
        .await
    })
}

#[test]
fn delete_uses_exact_route_and_authority_and_accepts_deleted_or_absent() {
    for (status, expected) in [
        ("204 No Content", DeleteOutcome::Deleted),
        ("404 Not Found", DeleteOutcome::AlreadyAbsent),
    ] {
        let (base, server) = scripted_server(vec![
            ("200 OK".into(), live_message()),
            (status.into(), String::new()),
        ]);
        let client = DiscordDeleteClient::for_test(&base).unwrap();
        let result = runtime().block_on(client.delete_owned(
            "9007199254741101",
            "1985931830091579393",
            "9007199254741001",
            &binding(),
            TOKEN,
            &AtomicBool::new(false),
        ));
        assert_eq!(result.unwrap(), expected);
        let requests = server.join().unwrap();
        let request = &requests[1];
        assert!(request.starts_with(
            "DELETE /api/v10/channels/9007199254741101/messages/1985931830091579393 HTTP/1.1"
        ));
        assert!(
            request
                .lines()
                .any(|line| line.eq_ignore_ascii_case(&format!("authorization: {TOKEN}")))
        );
    }
}

#[test]
fn authentication_permission_and_rate_limits_have_closed_outcomes() {
    type Case<'a> = (
        &'a str,
        Vec<(&'a str, &'a str)>,
        &'a str,
        DiscordDeleteError,
    );
    let cases: Vec<Case<'_>> = vec![
        (
            "401 Unauthorized",
            vec![],
            "",
            DiscordDeleteError::Authentication,
        ),
        ("403 Forbidden", vec![], "", DiscordDeleteError::Permission),
        (
            "429 Too Many Requests",
            vec![("Content-Type", "application/json")],
            r#"{"retry_after":0.25,"global":true}"#,
            DiscordDeleteError::RateLimited {
                retry_after_millis: 250,
                global: true,
            },
        ),
    ];
    for (status, headers, body, expected) in cases {
        let (result, _) = run_verify(status, &headers, body);
        assert_eq!(result.unwrap_err(), expected);
    }
}

#[test]
fn deletion_proves_the_exact_live_message_owner_before_issuing_delete() {
    let (base, server) = scripted_server(vec![
        ("200 OK".into(), live_message()),
        ("204 No Content".into(), String::new()),
    ]);
    let runtime = runtime();
    let client = DiscordDeleteClient::for_test(&base).unwrap();

    assert_eq!(
        runtime.block_on(client.delete_owned(
            "9007199254741101",
            "1985931830091579393",
            "9007199254741001",
            &binding(),
            TOKEN,
            &AtomicBool::new(false),
        )),
        Ok(DeleteOutcome::Deleted)
    );
    let requests = server.join().unwrap();
    assert!(requests[0].starts_with(
        "GET /api/v10/channels/9007199254741101/messages/1985931830091579393 HTTP/1.1"
    ));
    assert!(requests[1].starts_with(
        "DELETE /api/v10/channels/9007199254741101/messages/1985931830091579393 HTTP/1.1"
    ));
    for request in &requests {
        assert!(
            request
                .lines()
                .any(|line| line.eq_ignore_ascii_case(&format!("authorization: {TOKEN}")))
        );
    }
}

#[test]
fn live_message_identity_mismatch_fails_closed_before_delete() {
    for body in [
        r#"{"id":"1985931830091579394","channel_id":"9007199254741101","author":{"id":"9007199254741001"},"content":"synthetic message","timestamp":"2030-01-02T03:04:05.123Z"}"#,
        r#"{"id":"1985931830091579393","channel_id":"9007199254741102","author":{"id":"9007199254741001"},"content":"synthetic message","timestamp":"2030-01-02T03:04:05.123Z"}"#,
        r#"{"id":"1985931830091579393","channel_id":"9007199254741101","author":{"id":"9007199254741002"},"content":"synthetic message","timestamp":"2030-01-02T03:04:05.123Z"}"#,
        r#"{"id":"1985931830091579393","channel_id":"9007199254741101","author":{"id":"9007199254741001"},"content":"attacker-selected description","timestamp":"2030-01-02T03:04:05.123Z"}"#,
        r#"{"id":"1985931830091579393","channel_id":"9007199254741101","author":{"id":"9007199254741001"},"content":"synthetic message","timestamp":"2030-01-02T03:04:06.123Z"}"#,
        r#"{"id":"1985931830091579393","channel_id":"9007199254741101","author":{"id":"9007199254741001"},"content":"synthetic message","timestamp":"2030-01-02T03:04:05.123Z","attachments":[{"filename":"misleading.pdf"}]}"#,
        r#"{"id":"1985931830091579393"}"#,
    ] {
        let (base, server) = scripted_server(vec![("200 OK".into(), body.into())]);
        let runtime = runtime();
        let client = DiscordDeleteClient::for_test(&base).unwrap();
        assert_eq!(
            runtime.block_on(client.verify_owned(
                "9007199254741101",
                "1985931830091579393",
                "9007199254741001",
                &binding(),
                TOKEN,
                &AtomicBool::new(false),
            )),
            Err(DiscordDeleteError::OwnershipMismatch)
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET "));
    }
}

#[test]
fn delete_rechecks_reviewed_content_and_timestamp_before_mutation() {
    for body in [
        r#"{"id":"1985931830091579393","channel_id":"9007199254741101","author":{"id":"9007199254741001"},"content":"different live content","timestamp":"2030-01-02T03:04:05.123Z"}"#,
        r#"{"id":"1985931830091579393","channel_id":"9007199254741101","author":{"id":"9007199254741001"},"content":"synthetic message","timestamp":"2030-01-02T03:04:06.123Z"}"#,
    ] {
        let (base, server) = scripted_server(vec![("200 OK".into(), body.into())]);
        let client = DiscordDeleteClient::for_test(&base).unwrap();

        assert_eq!(
            runtime().block_on(client.delete_owned(
                "9007199254741101",
                "1985931830091579393",
                "9007199254741001",
                &binding(),
                TOKEN,
                &AtomicBool::new(false),
            )),
            Err(DiscordDeleteError::OwnershipMismatch)
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET "));
    }
}

#[test]
fn missing_live_message_is_not_counted_as_an_already_successful_delete() {
    let (base, server) = scripted_server(vec![("404 Not Found".into(), String::new())]);
    let runtime = runtime();
    let client = DiscordDeleteClient::for_test(&base).unwrap();

    assert_eq!(
        runtime.block_on(client.delete_owned(
            "9007199254741101",
            "1985931830091579393",
            "9007199254741001",
            &binding(),
            TOKEN,
            &AtomicBool::new(false),
        )),
        Err(DiscordDeleteError::NotFound)
    );
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET "));
}

#[test]
fn rate_limit_rejects_oversized_content_length_without_waiting_for_the_body() {
    let response = concat!(
        "HTTP/1.1 429 Too Many Requests\r\n",
        "Content-Type: application/json\r\n",
        "Content-Length: 4097\r\n",
        "Connection: close\r\n\r\n"
    );
    let (base, server) = stalled_rate_limit_server(response.into(), Vec::new());

    assert_eq!(
        run_verify_with_timeout(&base).unwrap(),
        Err(DiscordDeleteError::Transient)
    );
    server.join().unwrap();
}

#[test]
fn rate_limit_stops_streaming_once_the_bounded_body_limit_is_exceeded() {
    let response = concat!(
        "HTTP/1.1 429 Too Many Requests\r\n",
        "Content-Type: application/json\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Connection: close\r\n\r\n"
    );
    let body = vec![b'a'; 4097];
    let mut chunk = format!("{:x}\r\n", body.len()).into_bytes();
    chunk.extend_from_slice(&body);
    chunk.extend_from_slice(b"\r\n");
    let (base, server) = stalled_rate_limit_server(response.into(), chunk);

    assert_eq!(
        run_verify_with_timeout(&base).unwrap(),
        Err(DiscordDeleteError::Transient)
    );
    server.join().unwrap();
}

#[test]
fn live_message_probe_rejects_oversized_content_length_without_waiting_for_the_body() {
    let response = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: application/json\r\n",
        "Content-Length: 65537\r\n",
        "Connection: close\r\n\r\n"
    );
    let (base, server) = stalled_rate_limit_server(response.into(), Vec::new());

    assert_eq!(
        run_verify_with_timeout(&base).unwrap(),
        Err(DiscordDeleteError::Transient)
    );
    server.join().unwrap();
}

#[test]
fn invalid_ids_and_preflight_cancellation_never_open_a_connection() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let client = DiscordDeleteClient::for_test("http://127.0.0.1:9/api/v10/").unwrap();
    assert_eq!(
        runtime.block_on(client.verify_owned(
            "01",
            "2",
            "3",
            &binding(),
            TOKEN,
            &AtomicBool::new(false),
        )),
        Err(DiscordDeleteError::InvalidTarget)
    );
    assert_eq!(
        runtime.block_on(client.verify_owned(
            "1",
            "2",
            "3",
            &binding(),
            TOKEN,
            &AtomicBool::new(true),
        )),
        Err(DiscordDeleteError::Cancelled)
    );
}

#[test]
fn server_failures_retry_a_bounded_number_of_times_and_end_uncertain() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let (base, recovered) = scripted_server(vec![
        ("200 OK".into(), live_message()),
        ("503 Service Unavailable".into(), String::new()),
        ("200 OK".into(), live_message()),
        ("204 No Content".into(), String::new()),
    ]);
    let client = DiscordDeleteClient::for_test(&base).unwrap();
    assert_eq!(
        runtime.block_on(client.delete_owned(
            "9007199254741101",
            "1985931830091579393",
            "9007199254741001",
            &binding(),
            TOKEN,
            &AtomicBool::new(false),
        )),
        Ok(DeleteOutcome::Deleted)
    );
    assert_eq!(recovered.join().unwrap().len(), 4);

    let (base, exhausted) = scripted_server(vec![
        ("200 OK".into(), live_message()),
        ("503 Service Unavailable".into(), String::new()),
        ("200 OK".into(), live_message()),
        ("503 Service Unavailable".into(), String::new()),
        ("200 OK".into(), live_message()),
        ("503 Service Unavailable".into(), String::new()),
    ]);
    let client = DiscordDeleteClient::for_test(&base).unwrap();
    assert_eq!(
        runtime.block_on(client.delete_owned(
            "9007199254741101",
            "1985931830091579393",
            "9007199254741001",
            &binding(),
            TOKEN,
            &AtomicBool::new(false),
        )),
        Err(DiscordDeleteError::Ambiguous)
    );
    assert_eq!(exhausted.join().unwrap().len(), 6);
}
