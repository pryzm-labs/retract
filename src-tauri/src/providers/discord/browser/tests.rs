use super::BrowserFamily;
use super::discovery::{BrowserEnvironment, discover_with};
use super::protocol::ProtocolParser;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Default)]
struct FakeEnvironment {
    files: HashSet<PathBuf>,
    commands: HashMap<String, PathBuf>,
}

impl BrowserEnvironment for FakeEnvironment {
    fn is_file(&self, path: &Path) -> bool {
        self.files.contains(path)
    }

    fn find_command(&self, command: &str) -> Option<PathBuf> {
        self.commands.get(command).cloned()
    }
}

#[test]
fn discovery_is_cross_browser_deterministic_and_never_requests_a_profile() {
    let chrome = PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    let firefox = PathBuf::from("/Applications/Firefox.app/Contents/MacOS/firefox");
    let mut environment = FakeEnvironment::default();
    environment.files.extend([chrome.clone(), firefox.clone()]);
    environment.commands.insert("google-chrome".into(), chrome);

    let found = discover_with(&environment, "macos");
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].id, "chrome");
    assert_eq!(found[0].family, BrowserFamily::ChromiumCdp);
    assert_eq!(found[1].id, "firefox");
    assert_eq!(found[1].family, BrowserFamily::FirefoxBidi);
    assert!(
        found
            .iter()
            .all(|browser| !browser.executable.to_string_lossy().contains("Profile"))
    );
}

#[test]
fn discovery_supports_chromium_variants_firefox_and_path_fallbacks() {
    let mut environment = FakeEnvironment::default();
    for command in [
        "microsoft-edge",
        "brave-browser",
        "vivaldi",
        "opera",
        "chromium",
        "firefox",
    ] {
        environment
            .commands
            .insert(command.into(), PathBuf::from(format!("/usr/bin/{command}")));
    }
    let found = discover_with(&environment, "linux");
    assert_eq!(
        found
            .iter()
            .map(|browser| browser.id.as_str())
            .collect::<Vec<_>>(),
        ["edge", "brave", "vivaldi", "opera", "chromium", "firefox"]
    );
    assert_eq!(found.last().unwrap().family, BrowserFamily::FirefoxBidi);
}

#[test]
fn cdp_accepts_only_discord_api_authorization_and_correlates_extra_info() {
    let mut parser = ProtocolParser::default();
    let token = "synthetic.discord.token-value_123456789";
    assert!(parser
        .parse_cdp(r#"{"method":"Network.requestWillBeSent","params":{"requestId":"r1","request":{"url":"https://discord.com/api/v9/users/@me","headers":{}}}}"#)
        .unwrap()
        .is_none());
    let captured = parser
        .parse_cdp(&format!(r#"{{"method":"Network.requestWillBeSentExtraInfo","params":{{"requestId":"r1","headers":{{"Authorization":"{token}"}}}}}}"#))
        .unwrap()
        .unwrap();
    assert_eq!(captured.expose(|value| value.to_owned()), token);
}

#[test]
fn bidi_accepts_string_headers_but_rejects_origins_and_unsafe_values() {
    let token = "synthetic.discord.token-value_123456789";
    let mut parser = ProtocolParser::default();
    let accepted = format!(
        r#"{{"method":"network.beforeRequestSent","params":{{"request":{{"url":"https://discord.com/api/v10/users/@me","headers":[{{"name":"authorization","value":{{"type":"string","value":"{token}"}}}}]}}}}}}"#
    );
    assert_eq!(
        parser
            .parse_bidi(&accepted)
            .unwrap()
            .unwrap()
            .expose(str::to_owned),
        token
    );

    for event in [
        format!(r#"{{"method":"network.beforeRequestSent","params":{{"request":{{"url":"https://discord.com.evil.test/api/v10/users/@me","headers":[{{"name":"authorization","value":"{token}"}}]}}}}}}"#),
        r#"{"method":"network.beforeRequestSent","params":{"request":{"url":"https://discord.com/channels/@me","headers":[{"name":"authorization","value":"synthetic.discord.token-value_123456789"}]}}}"#.into(),
        r#"{"method":"network.beforeRequestSent","params":{"request":{"url":"https://discord.com/api/v10/users/@me","headers":[{"name":"authorization","value":"Bot abcdefghijklmnopqrstuvwxyz"}]}}}"#.into(),
    ] {
        assert!(parser.parse_bidi(&event).unwrap().is_none());
    }
}

#[test]
fn malformed_events_and_cookie_headers_never_capture_or_echo_values() {
    let mut parser = ProtocolParser::default();
    for event in [
        "not json",
        r#"{"method":"Network.requestWillBeSent","params":{"requestId":"r","request":{"url":"https://discord.com/api/v9/users/@me","headers":{"Cookie":"secret"}}}}"#,
        r#"{"method":"Network.requestWillBeSent","params":{"requestId":"r","request":{"url":"http://discord.com/api/v9/users/@me","headers":{"Authorization":"synthetic.discord.token-value_123456789"}}}}"#,
    ] {
        assert!(parser.parse_cdp(event).unwrap_or(None).is_none());
    }
}

#[test]
fn launch_arguments_use_only_new_profiles_loopback_protocols_and_discord_login() {
    let profile = Path::new("/private/tmp/retract-new-profile");
    let chromium = super::chromium::launch_arguments(profile);
    let chromium = chromium
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(
        chromium
            .iter()
            .any(|value| value == "--remote-debugging-address=127.0.0.1")
    );
    assert!(
        chromium
            .iter()
            .any(|value| value == "--remote-debugging-port=0")
    );
    assert!(
        chromium
            .iter()
            .any(|value| value == "--user-data-dir=/private/tmp/retract-new-profile")
    );
    assert_eq!(chromium.last().unwrap(), "https://discord.com/app");

    let firefox = super::firefox::launch_arguments(profile, 49152);
    let firefox = firefox
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>();
    assert_eq!(
        firefox,
        [
            "--profile",
            "/private/tmp/retract-new-profile",
            "--remote-debugging-port",
            "49152",
            "--new-instance",
            "https://discord.com/app",
        ]
    );
}
