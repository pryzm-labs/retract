use super::http::{DeleteOutcome, DiscordDeleteClient, DiscordDeleteError};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::thread;

const TOKEN: &str = "synthetic.discord.token-value_123456789";

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

fn run_delete(
    status: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (Result<DeleteOutcome, DiscordDeleteError>, String) {
    let (base, handle) = server(status, headers, body);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let client = DiscordDeleteClient::for_test(&base).unwrap();
    let result = runtime.block_on(client.delete(
        "9007199254741101",
        "1985931830091579393",
        TOKEN,
        &AtomicBool::new(false),
    ));
    (result, handle.join().unwrap())
}

#[test]
fn delete_uses_exact_route_and_authority_and_accepts_deleted_or_absent() {
    for (status, expected) in [
        ("204 No Content", DeleteOutcome::Deleted),
        ("404 Not Found", DeleteOutcome::AlreadyAbsent),
    ] {
        let (result, request) = run_delete(status, &[], "");
        assert_eq!(result.unwrap(), expected);
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
        let (result, _) = run_delete(status, &headers, body);
        assert_eq!(result.unwrap_err(), expected);
    }
}

#[test]
fn invalid_ids_and_preflight_cancellation_never_open_a_connection() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let client = DiscordDeleteClient::for_test("http://127.0.0.1:9/api/v10/").unwrap();
    assert_eq!(
        runtime.block_on(client.delete("01", "2", TOKEN, &AtomicBool::new(false))),
        Err(DiscordDeleteError::InvalidTarget)
    );
    assert_eq!(
        runtime.block_on(client.delete("1", "2", TOKEN, &AtomicBool::new(true))),
        Err(DiscordDeleteError::Cancelled)
    );
}
