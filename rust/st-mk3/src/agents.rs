//! Native Instruments' background agents on macOS.
//!
//! NIHardwareAgent and NIHostIntegrationAgent hold the Maschine's display interface whenever the
//! controller is connected. To draw the screens they are stopped for this user's session, and
//! started again afterwards. What was stopped is written to a file first, so that a crash in
//! between still leads to their restoration at the next launch.
//!
//! The process control itself ([`NiAgents`]) exists on macOS only; the parsing it relies on is
//! plain text handling and is tested everywhere.

use std::path::{Path, PathBuf};

use crate::transport::AgentRecord;

/// The agents: `(process name, fragment of the launchd label, application bundle)`.
/// NIHardwareAgent's bundle identifier is NIHardwareService.
pub const AGENTS: [(&str, &str, &str); 2] = [
    ("NIHostIntegrationAgent", "com.native-instruments.nihostintegrationagent", "/Library/Application Support/Native Instruments/Hardware/NIHostIntegrationAgent.app"),
    ("NIHardwareAgent", "com.native-instruments.nihardwareservice", "/Library/Application Support/Native Instruments/Hardware/NIHardwareAgent.app"),
];

/// A launchd job of this session whose label mentions Native Instruments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchdJob {
    /// Its process id, when it runs.
    pub pid: Option<u32>,
    /// Its label.
    pub label: String,
}

/// Reads the output of `launchctl list` (tab-separated `pid`, `status`, `label`) and keeps the
/// jobs whose label contains "native".
pub fn parse_launchd_jobs(listing: &str) -> Vec<LaunchdJob> {
    listing
        .lines()
        .filter_map(|line| {
            let columns: Vec<&str> = line.split('\t').collect();
            (columns.len() >= 3 && columns[2].to_lowercase().contains("native")).then(|| LaunchdJob { pid: columns[0].trim().parse().ok(), label: columns[2].to_string() })
        })
        .collect()
}

/// Finds the property list of a job in the output of `launchctl print <domain>/<label>`.
pub fn parse_plist_path(print: &str) -> Option<String> {
    print.lines().map(str::trim).find_map(|line| line.strip_prefix("path = ")).filter(|path| path.ends_with(".plist")).map(str::to_string)
}

/// Reads the record of stopped agents; a missing or unreadable file is "none".
pub fn read_state(path: &Path) -> Vec<AgentRecord> {
    std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Writes the record of stopped agents, replacing the file in one step.
pub fn write_state(path: &Path, records: &[AgentRecord]) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("The record of paused Native Instruments agents could not be written ({}): {error}", path.display());
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(failed)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(records).unwrap_or_default()).map_err(failed)?;
    std::fs::rename(&temporary, path).map_err(failed)
}

/// Where the record lives inside the application's support folder.
pub fn state_path(support_dir: &Path) -> PathBuf {
    support_dir.join("MaschineMK3").join("stopped-ni-agents.json")
}

/// The records to keep for a take-over: what an earlier run already recorded, plus every agent
/// that runs now (as a process, or as a launchd job with a process id).
pub fn records_to_stop(recorded: Vec<AgentRecord>, jobs: &[LaunchdJob], is_running: &dyn Fn(&str) -> bool, plist_path: &dyn Fn(&str) -> Option<String>) -> Vec<AgentRecord> {
    let mut records = recorded;
    for (name, fragment, app_path) in AGENTS {
        let job = jobs.iter().find(|job| job.label.to_lowercase().contains(fragment));
        if !(is_running(name) || job.is_some_and(|job| job.pid.is_some())) {
            continue;
        }
        if records.iter().any(|record| record.name == name) {
            continue;
        }
        let label = job.map(|job| job.label.clone());
        // An "application." job was started by opening the bundle; it has no property list.
        let plist = label.as_deref().filter(|label| !label.starts_with("application.")).and_then(plist_path);
        records.push(AgentRecord { name: name.to_string(), label, app_path: app_path.to_string(), plist_path: plist });
    }
    records
}

/// The order agents are started in: NIHardwareAgent first.
pub fn restore_order(records: &[AgentRecord]) -> Vec<&AgentRecord> {
    records.iter().filter(|record| record.name == "NIHardwareAgent").chain(records.iter().filter(|record| record.name != "NIHardwareAgent")).collect()
}

#[cfg(target_os = "macos")]
pub use macos::NiAgents;

#[cfg(target_os = "macos")]
mod macos {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use super::{parse_launchd_jobs, parse_plist_path, read_state, records_to_stop, restore_order, write_state, LaunchdJob};
    use crate::transport::{AgentRecord, VendorAgents};

    /// Stops and restores Native Instruments' agents with `launchctl`, `pgrep`, `kill` and `open`.
    pub struct NiAgents {
        state: PathBuf,
    }

    fn run(program: &str, arguments: &[&str]) -> (i32, String) {
        match Command::new(program).args(arguments).output() {
            Ok(output) => (output.status.code().unwrap_or(-1), String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr)),
            Err(error) => (-1, error.to_string()),
        }
    }

    fn user_id() -> String {
        run("/usr/bin/id", &["-u"]).1.trim().to_string()
    }

    fn domain() -> String {
        format!("gui/{}", user_id())
    }

    fn pids(name: &str) -> Vec<String> {
        run("/usr/bin/pgrep", &["-x", "-U", &user_id(), name]).1.lines().map(|line| line.trim().to_string()).filter(|line| !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit())).collect()
    }

    fn is_running(name: &str) -> bool {
        !pids(name).is_empty()
    }

    fn signal(name: &str, signal: &str) {
        for pid in pids(name) {
            run("/bin/kill", &[signal, &pid]);
        }
    }

    fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline && !done() {
            sleep(Duration::from_millis(100));
        }
    }

    impl NiAgents {
        /// Agents whose record of stopped ones is kept at `state` (see [`super::state_path`]).
        pub fn new(state: impl Into<PathBuf>) -> Self {
            NiAgents { state: state.into() }
        }

        fn jobs() -> Vec<LaunchdJob> {
            parse_launchd_jobs(&run("/bin/launchctl", &["list"]).1)
        }

        fn plist_path(label: &str) -> Option<String> {
            parse_plist_path(&run("/bin/launchctl", &["print", &format!("{}/{label}", domain())]).1).filter(|path| Path::new(path).exists())
        }
    }

    impl VendorAgents for NiAgents {
        fn recover(&self, log: &dyn Fn(&str)) {
            let records = read_state(&self.state);
            if !records.is_empty() {
                log("restoring NI agents left stopped by an earlier run");
                self.restore(&records, log);
            }
        }

        fn stop(&self, log: &dyn Fn(&str)) -> Result<Vec<AgentRecord>, String> {
            let records = records_to_stop(read_state(&self.state), &Self::jobs(), &is_running, &Self::plist_path);
            if records.is_empty() {
                log("NI agents are not running");
                return Ok(Vec::new());
            }
            // On disk before anything is stopped: a crash from here on still restores them.
            write_state(&self.state, &records)?;
            let domain = domain();
            for attempt in 0..3 {
                for record in records.iter().filter(|record| is_running(&record.name)) {
                    if let Some(label) = &record.label {
                        if record.plist_path.is_some() {
                            // A job with a property list may be kept alive: boot it out so launchd does not respawn it.
                            let (status, _) = run("/bin/launchctl", &["bootout", &format!("{domain}/{label}")]);
                            log(&format!("launchctl bootout {label} → {status}"));
                        } else {
                            let (status, _) = run("/bin/launchctl", &["kill", "SIGTERM", &format!("{domain}/{label}")]);
                            log(&format!("launchctl kill SIGTERM {label} → {status}"));
                        }
                    }
                    signal(&record.name, "-TERM");
                }
                wait_until(Duration::from_secs(3), || !records.iter().any(|record| is_running(&record.name)));
                for record in &records {
                    if is_running(&record.name) {
                        log(&format!("SIGKILL {}", record.name));
                        signal(&record.name, "-KILL");
                    }
                }
                sleep(Duration::from_millis(if attempt == 0 { 800 } else { 400 }));
                // Some of these components relaunch each other: repeat until none came back.
                if !records.iter().any(|record| is_running(&record.name)) {
                    break;
                }
                log("NI agents came back; stopping again");
            }
            let survivors: Vec<&str> = records.iter().filter(|record| is_running(&record.name)).map(|record| record.name.as_str()).collect();
            if !survivors.is_empty() {
                return Err(format!("Could not stop {}.", survivors.join(", ")));
            }
            log(&format!("stopped {}", records.iter().map(|record| record.name.as_str()).collect::<Vec<_>>().join(", ")));
            Ok(records)
        }

        fn restore(&self, records: &[AgentRecord], log: &dyn Fn(&str)) -> bool {
            let domain = domain();
            for record in restore_order(records) {
                if is_running(&record.name) {
                    continue;
                }
                if let (Some(plist), Some(label)) = (&record.plist_path, &record.label) {
                    let (boot, _) = run("/bin/launchctl", &["bootstrap", &domain, plist]);
                    let (kick, _) = run("/bin/launchctl", &["kickstart", &format!("{domain}/{label}")]);
                    log(&format!("launchctl bootstrap/kickstart {label} → {boot}/{kick}"));
                } else if Path::new(&record.app_path).exists() {
                    let (status, _) = run("/usr/bin/open", &["-g", "-j", &record.app_path]);
                    log(&format!("open {} → {status}", record.name));
                }
                wait_until(Duration::from_secs(4), || is_running(&record.name));
            }
            let missing: Vec<&str> = records.iter().filter(|record| !is_running(&record.name)).map(|record| record.name.as_str()).collect();
            if missing.is_empty() {
                let _ = std::fs::remove_file(&self.state);
                log("NI agents restored");
                true
            } else {
                log(&format!("could not restart {}", missing.join(", ")));
                false
            }
        }

        fn read_state(&self) -> Vec<AgentRecord> {
            read_state(&self.state)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_listings_are_read() {
        let listing = "PID\tStatus\tLabel\n501\t0\tcom.apple.Finder\n812\t0\tcom.native-instruments.NIHostIntegrationAgent\n-\t0\tapplication.com.native-instruments.NIHardwareService.123.456\nbroken line\n";
        assert_eq!(
            parse_launchd_jobs(listing),
            [
                LaunchdJob { pid: Some(812), label: "com.native-instruments.NIHostIntegrationAgent".into() },
                LaunchdJob { pid: None, label: "application.com.native-instruments.NIHardwareService.123.456".into() },
            ]
        );
        let print = "com.x = {\n\tactive count = 1\n\tpath = /Library/LaunchAgents/com.native-instruments.NIHostIntegrationAgent.plist\n\tstate = running\n}";
        assert_eq!(parse_plist_path(print).as_deref(), Some("/Library/LaunchAgents/com.native-instruments.NIHostIntegrationAgent.plist"));
        assert_eq!(parse_plist_path("path = /Applications/Thing.app\n"), None);
        assert_eq!(parse_plist_path(""), None);
    }

    #[test]
    fn the_agents_to_stop_are_those_running_plus_those_already_recorded() {
        let jobs = parse_launchd_jobs("812\t0\tcom.native-instruments.NIHostIntegrationAgent\n-\t0\tapplication.com.native-instruments.NIHardwareService.1.2\n");
        let plist = |label: &str| Some(format!("/Library/LaunchAgents/{label}.plist"));
        // The job with a process id is stopped although no process of that name was found; the
        // other agent runs as a process started from its bundle.
        let records = records_to_stop(Vec::new(), &jobs, &|name| name == "NIHardwareAgent", &plist);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "NIHostIntegrationAgent");
        assert_eq!(records[0].plist_path.as_deref(), Some("/Library/LaunchAgents/com.native-instruments.NIHostIntegrationAgent.plist"));
        assert_eq!(records[1].name, "NIHardwareAgent");
        assert_eq!(records[1].label.as_deref(), Some("application.com.native-instruments.NIHardwareService.1.2"));
        assert_eq!(records[1].plist_path, None, "an application job has no property list");
        assert!(records[1].app_path.ends_with("NIHardwareAgent.app"));
        // Nothing runs: nothing to stop. An earlier record is kept and not duplicated.
        assert!(records_to_stop(Vec::new(), &[], &|_| false, &plist).is_empty());
        let earlier = vec![AgentRecord { name: "NIHardwareAgent".into(), ..AgentRecord::default() }];
        let kept = records_to_stop(earlier.clone(), &jobs, &|_| true, &plist);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0], earlier[0]);
        assert_eq!(restore_order(&records).iter().map(|record| record.name.as_str()).collect::<Vec<_>>(), ["NIHardwareAgent", "NIHostIntegrationAgent"]);
    }

    #[test]
    fn the_record_survives_on_disk() {
        let folder = std::env::temp_dir().join(format!("st-mk3-agents-{}", std::process::id()));
        let path = state_path(&folder);
        assert!(path.ends_with("MaschineMK3/stopped-ni-agents.json"));
        assert!(read_state(&path).is_empty(), "no file: nothing stopped");
        let records = vec![AgentRecord { name: "NIHardwareAgent".into(), label: None, app_path: "/x.app".into(), plist_path: Some("/y.plist".into()) }];
        write_state(&path, &records).unwrap();
        assert_eq!(read_state(&path), records);
        std::fs::write(&path, b"not json").unwrap();
        assert!(read_state(&path).is_empty(), "an unreadable record is no record");
        let _ = std::fs::remove_dir_all(&folder);
    }
}
