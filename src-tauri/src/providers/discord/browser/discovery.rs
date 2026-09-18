use super::{BrowserDescriptor, BrowserFamily};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(crate) trait BrowserEnvironment {
    fn is_file(&self, path: &Path) -> bool;
    fn find_command(&self, command: &str) -> Option<PathBuf>;
}

pub(crate) struct SystemBrowserEnvironment;

impl BrowserEnvironment for SystemBrowserEnvironment {
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn find_command(&self, command: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|directory| directory.join(command))
            .find(|candidate| candidate.is_file())
    }
}

struct Candidate {
    id: &'static str,
    name: &'static str,
    family: BrowserFamily,
    macos: Option<&'static str>,
    linux: &'static [&'static str],
    windows: &'static [&'static str],
}

const CANDIDATES: &[Candidate] = &[
    Candidate {
        id: "chrome",
        name: "Google Chrome",
        family: BrowserFamily::ChromiumCdp,
        macos: Some("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        linux: &["google-chrome", "google-chrome-stable"],
        windows: &["chrome.exe"],
    },
    Candidate {
        id: "edge",
        name: "Microsoft Edge",
        family: BrowserFamily::ChromiumCdp,
        macos: Some("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
        linux: &["microsoft-edge", "microsoft-edge-stable"],
        windows: &["msedge.exe"],
    },
    Candidate {
        id: "brave",
        name: "Brave",
        family: BrowserFamily::ChromiumCdp,
        macos: Some("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"),
        linux: &["brave-browser", "brave-browser-stable"],
        windows: &["brave.exe"],
    },
    Candidate {
        id: "arc",
        name: "Arc",
        family: BrowserFamily::ChromiumCdp,
        macos: Some("/Applications/Arc.app/Contents/MacOS/Arc"),
        linux: &[],
        windows: &["Arc.exe"],
    },
    Candidate {
        id: "vivaldi",
        name: "Vivaldi",
        family: BrowserFamily::ChromiumCdp,
        macos: Some("/Applications/Vivaldi.app/Contents/MacOS/Vivaldi"),
        linux: &["vivaldi", "vivaldi-stable"],
        windows: &["vivaldi.exe"],
    },
    Candidate {
        id: "opera",
        name: "Opera",
        family: BrowserFamily::ChromiumCdp,
        macos: Some("/Applications/Opera.app/Contents/MacOS/Opera"),
        linux: &["opera"],
        windows: &["opera.exe"],
    },
    Candidate {
        id: "chromium",
        name: "Chromium",
        family: BrowserFamily::ChromiumCdp,
        macos: Some("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        linux: &["chromium", "chromium-browser"],
        windows: &["chromium.exe"],
    },
    Candidate {
        id: "firefox",
        name: "Firefox",
        family: BrowserFamily::FirefoxBidi,
        macos: Some("/Applications/Firefox.app/Contents/MacOS/firefox"),
        linux: &["firefox"],
        windows: &["firefox.exe"],
    },
];

pub(crate) fn discover() -> Vec<BrowserDescriptor> {
    discover_with(&SystemBrowserEnvironment, std::env::consts::OS)
}

pub(crate) fn discover_with(
    environment: &impl BrowserEnvironment,
    platform: &str,
) -> Vec<BrowserDescriptor> {
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for candidate in CANDIDATES {
        let configured = match platform {
            "macos" => candidate
                .macos
                .map(PathBuf::from)
                .filter(|path| environment.is_file(path)),
            "windows" => candidate
                .windows
                .iter()
                .find_map(|command| environment.find_command(command)),
            _ => candidate
                .linux
                .iter()
                .find_map(|command| environment.find_command(command)),
        };
        let Some(executable) = configured.or_else(|| {
            candidate
                .linux
                .iter()
                .chain(candidate.windows.iter())
                .find_map(|command| environment.find_command(command))
        }) else {
            continue;
        };
        if !seen.insert(executable.clone()) {
            continue;
        }
        found.push(BrowserDescriptor {
            id: candidate.id.into(),
            display_name: candidate.name.into(),
            family: candidate.family,
            executable,
        });
    }
    found
}
