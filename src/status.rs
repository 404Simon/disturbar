use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::Duration;

use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceStatus {
    labels: Vec<MonitorWorkspaceLabel>,
    fallback: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MonitorWorkspaceLabel {
    monitor: String,
    label: String,
}

pub struct BarStatus {
    pub workspaces: WorkspaceStatus,
    pub song: String,
    pub battery: String,
    pub volume: String,
    pub datetime: String,
}

#[derive(Debug, Clone)]
pub enum StatusEvent {
    WorkspaceDirty,
}

pub struct StatusEventStreams {
    pub rx: Receiver<StatusEvent>,
    pub workspace_event_driven: bool,
}

impl WorkspaceStatus {
    pub fn label_for_monitor(&self, monitor_name: Option<&str>) -> &str {
        if let Some(monitor_name) = monitor_name
            && let Some(label) = self
                .labels
                .iter()
                .find(|entry| entry.monitor == monitor_name)
                .map(|entry| entry.label.as_str())
        {
            return label;
        }

        &self.fallback
    }
}

impl BarStatus {
    pub fn gather(detail_mode: bool) -> Self {
        Self {
            workspaces: format_workspaces(),
            song: gather_song(),
            battery: gather_battery(detail_mode),
            volume: gather_volume(detail_mode),
            datetime: gather_datetime(),
        }
    }

    pub fn gather_workspaces() -> WorkspaceStatus {
        format_workspaces()
    }
}

pub fn gather_battery(detail_mode: bool) -> String {
    format_battery(detail_mode)
}

pub fn gather_song() -> String {
    format_song()
}

pub fn gather_volume(detail_mode: bool) -> String {
    format_volume(detail_mode)
}

pub fn gather_datetime() -> String {
    format_datetime()
}

pub fn spawn_status_event_streams() -> StatusEventStreams {
    const STATUS_EVENT_BUFFER: usize = 32;
    let (tx, rx) = mpsc::sync_channel(STATUS_EVENT_BUFFER);

    let workspace_event_driven = spawn_workspace_listener(tx);

    StatusEventStreams {
        rx,
        workspace_event_driven,
    }
}

fn spawn_workspace_listener(tx: SyncSender<StatusEvent>) -> bool {
    let Some(sig) = hyprland_signature() else {
        return false;
    };
    let Some(socket_path) = hyprland_event_socket_path(&sig) else {
        return false;
    };

    thread::spawn(move || {
        loop {
            let Ok(stream) = UnixStream::connect(&socket_path) else {
                thread::sleep(Duration::from_secs(1));
                continue;
            };

            let reader = BufReader::new(stream);
            for line in reader.lines().map_while(Result::ok) {
                if !is_workspace_event(&line) {
                    continue;
                }
                send_dirty_event(&tx, StatusEvent::WorkspaceDirty);
            }

            thread::sleep(Duration::from_millis(250));
        }
    });

    true
}

fn hyprland_signature() -> Option<String> {
    std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .ok()
        .filter(|sig| !sig.is_empty())
        .or_else(discover_hyprland_signature)
}

fn hyprland_event_socket_path(sig: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        candidates.push(
            PathBuf::from(runtime)
                .join("hypr")
                .join(sig)
                .join(".socket2.sock"),
        );
    }
    candidates.push(PathBuf::from("/tmp/hypr").join(sig).join(".socket2.sock"));
    candidates.into_iter().find(|p| p.exists())
}

fn is_workspace_event(line: &str) -> bool {
    let Some((kind, _)) = line.split_once(">>") else {
        return false;
    };
    matches!(
        kind,
        "workspace"
            | "workspacev2"
            | "focusedmon"
            | "focusedmonv2"
            | "createworkspace"
            | "createworkspacev2"
            | "destroyworkspace"
            | "destroyworkspacev2"
            | "moveworkspace"
            | "moveworkspacev2"
            | "renameworkspace"
    )
}

fn send_dirty_event(tx: &SyncSender<StatusEvent>, event: StatusEvent) {
    let _ = tx.try_send(event);
}

fn format_workspaces() -> WorkspaceStatus {
    let active_raw = run_hyprctl(&["-j", "activeworkspace"]).unwrap_or_default();
    let monitors_raw = run_hyprctl(&["-j", "monitors"]).unwrap_or_default();
    let list_raw = run_hyprctl(&["-j", "workspaces"]).unwrap_or_default();

    let active_id = parse_json_i64_field(&active_raw, "id")
        .or_else(|| {
            parse_monitors(&monitors_raw)
                .iter()
                .find_map(|m| m.active_workspace_id)
        })
        .unwrap_or(1);

    let mut all_ids = parse_workspace_ids(&list_raw);
    all_ids.sort_unstable();
    all_ids.dedup();

    let fallback = if all_ids.is_empty() {
        "[1] 2 3 4 5".to_string()
    } else {
        format_workspace_label(&all_ids, Some(active_id))
    };

    let monitors = parse_monitors(&monitors_raw);
    let workspaces = parse_monitor_workspaces(&list_raw);

    let labels = monitors
        .into_iter()
        .map(|monitor| {
            let mut ids = workspaces
                .iter()
                .filter(|workspace| workspace.monitor == monitor.name)
                .map(|workspace| workspace.id)
                .filter(|id| *id > 0)
                .collect::<Vec<_>>();
            ids.sort_unstable();
            ids.dedup();

            if ids.is_empty()
                && let Some(active_id) = monitor.active_workspace_id
            {
                ids.push(active_id);
            }

            let label = if ids.is_empty() {
                fallback.clone()
            } else {
                format_workspace_label(&ids, monitor.active_workspace_id)
            };

            MonitorWorkspaceLabel {
                monitor: monitor.name,
                label,
            }
        })
        .collect();

    WorkspaceStatus { labels, fallback }
}

fn format_workspace_label(ids: &[i64], active_id: Option<i64>) -> String {
    ids.iter()
        .map(|id| {
            if Some(*id) == active_id {
                format!("[{id}]")
            } else {
                id.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_battery(detail_mode: bool) -> String {
    let state = read_battery_state();
    if detail_mode {
        return format_battery_time(state.as_ref());
    }

    let (value, charging) = state
        .map(|state| (state.capacity, state.charging))
        .unwrap_or_else(|| ("--".to_string(), false));
    if charging {
        format!("BAT CH {value}%")
    } else {
        format!("BAT {value}%")
    }
}

fn format_song() -> String {
    let raw = run_cmd("rmpc", &["song"]).unwrap_or_default();
    if raw.trim().is_empty() {
        return String::new();
    }

    let title = find_json_string_field(&raw, "title")
        .or_else(|| song_title_from_file(&raw))
        .unwrap_or_default();
    let artist = find_json_string_field(&raw, "artist").unwrap_or_default();

    let label = match (sanitize_bar_text(&title), sanitize_bar_text(&artist)) {
        (title, artist) if title.is_empty() && artist.is_empty() => String::new(),
        (title, artist) if artist.is_empty() => title,
        (title, artist) if title.is_empty() => artist,
        (title, artist) => format!("{artist} - {title}"),
    };

    label.trim().to_string()
}

fn format_volume(detail_mode: bool) -> String {
    let raw = run_cmd("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).unwrap_or_default();

    if detail_mode {
        return format_volume_device(&raw);
    }

    let value = parse_volume_percent(&raw).unwrap_or_else(|| "--".to_string());
    if is_volume_muted(&raw) {
        format!("VOL MUT {value}%")
    } else {
        format!("VOL {value}%")
    }
}

fn format_datetime() -> String {
    const FALLBACK: &str = "-- -- --- --:--";

    let mut now: libc::time_t = 0;
    // SAFETY: libc::time writes current time into valid pointer.
    unsafe {
        libc::time(&mut now as *mut libc::time_t);
    }

    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: localtime_r initializes tm when non-null returned.
    let ok = unsafe { !libc::localtime_r(&now as *const libc::time_t, tm.as_mut_ptr()).is_null() };
    if !ok {
        return FALLBACK.to_string();
    }

    // SAFETY: initialized by localtime_r check above.
    let tm = unsafe { tm.assume_init() };
    let mut out = [0_u8; 64];
    let fmt = b"%a %d %b %H:%M\0";

    // SAFETY: all pointers valid, fmt null-terminated.
    let written = unsafe {
        libc::strftime(
            out.as_mut_ptr() as *mut libc::c_char,
            out.len(),
            fmt.as_ptr() as *const libc::c_char,
            &tm as *const libc::tm,
        )
    };
    if written == 0 {
        return FALLBACK.to_string();
    }

    std::str::from_utf8(&out[..written])
        .map(|s| s.to_ascii_uppercase())
        .unwrap_or_else(|_| FALLBACK.to_string())
}

fn run_cmd(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

fn run_hyprctl(args: &[&str]) -> Option<String> {
    let bins = [
        "hyprctl",
        "/run/current-system/sw/bin/hyprctl",
        "/home/simon/.nix-profile/bin/hyprctl",
    ];

    for bin in bins {
        let mut cmd = Command::new(bin);
        cmd.args(args);

        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none()
            && let Some(sig) = discover_hyprland_signature()
        {
            cmd.env("HYPRLAND_INSTANCE_SIGNATURE", sig);
        }

        let Ok(out) = cmd.output() else {
            continue;
        };

        if !out.status.success() {
            continue;
        }

        let Ok(stdout) = String::from_utf8(out.stdout) else {
            continue;
        };

        if !stdout.trim().is_empty() {
            return Some(stdout);
        }
    }

    None
}

fn discover_hyprland_signature() -> Option<String> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let entries = fs::read_dir(PathBuf::from(runtime).join("hypr")).ok()?;

    let mut newest: Option<(std::time::SystemTime, String)> = None;

    for entry in entries.flatten() {
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }

        let sig = entry.file_name().to_string_lossy().to_string();
        if sig.is_empty() {
            continue;
        }

        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        if newest.as_ref().is_none_or(|(cur, _)| modified > *cur) {
            newest = Some((modified, sig));
        }
    }

    newest.map(|(_, sig)| sig)
}

struct BatteryState {
    capacity: String,
    charging: bool,
    energy_now: Option<f64>,
    energy_full: Option<f64>,
    power_now: Option<f64>,
}

fn read_battery_state() -> Option<BatteryState> {
    let entries = fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("BAT") {
            continue;
        }
        let base = entry.path();
        let cap_path = base.join("capacity");
        let status_path = base.join("status");

        let Ok(raw_capacity) = fs::read_to_string(cap_path) else {
            continue;
        };

        let capacity = raw_capacity.trim().to_string();
        if capacity.is_empty() {
            continue;
        }

        let charging = fs::read_to_string(status_path)
            .ok()
            .is_some_and(|status| is_battery_charging(&status));

        return Some(BatteryState {
            capacity,
            charging,
            energy_now: read_battery_metric(&base, &["energy_now", "charge_now"]),
            energy_full: read_battery_metric(&base, &["energy_full", "charge_full"]),
            power_now: read_battery_metric(&base, &["power_now", "current_now"]),
        });
    }
    None
}

fn read_battery_metric(base: &PathBuf, names: &[&str]) -> Option<f64> {
    for name in names {
        let Ok(raw) = fs::read_to_string(base.join(name)) else {
            continue;
        };
        let Ok(value) = raw.trim().parse::<f64>() else {
            continue;
        };
        if value > 0.0 {
            return Some(value);
        }
    }
    None
}

fn format_battery_time(state: Option<&BatteryState>) -> String {
    let Some(state) = state else {
        return "BAT --:--".to_string();
    };

    let Some(power_now) = state.power_now else {
        return battery_time_fallback_label(state.charging);
    };

    if power_now <= 0.0 {
        return battery_time_fallback_label(state.charging);
    }

    let hours = if state.charging {
        let Some(energy_full) = state.energy_full else {
            return battery_time_fallback_label(true);
        };
        let Some(energy_now) = state.energy_now else {
            return battery_time_fallback_label(true);
        };
        ((energy_full - energy_now).max(0.0)) / power_now
    } else {
        let Some(energy_now) = state.energy_now else {
            return battery_time_fallback_label(false);
        };
        energy_now / power_now
    };

    let total_minutes = (hours * 60.0).floor() as i64;
    let hh = (total_minutes / 60).max(0);
    let mm = (total_minutes % 60).max(0);
    if state.charging {
        format!("BAT CH {hh:02}:{mm:02}")
    } else {
        format!("BAT {hh:02}:{mm:02}")
    }
}

fn battery_time_fallback_label(charging: bool) -> String {
    if charging {
        "BAT CH --:--".to_string()
    } else {
        "BAT --:--".to_string()
    }
}

fn format_volume_device(volume_raw: &str) -> String {
    let raw = run_cmd("wpctl", &["inspect", "@DEFAULT_AUDIO_SINK@"]).unwrap_or_default();
    let label = parse_wpctl_device_name(&raw)
        .map(|name| format_volume_device_label(&sanitize_bar_text(&name)))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "--".to_string());
    if is_volume_muted(volume_raw) {
        format!("VOL MUT {label}")
    } else {
        format!("VOL {label}")
    }
}

fn parse_volume_percent(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|part| {
        let trimmed = part.trim();
        if let Ok(value) = trimmed.parse::<f32>() {
            let pct = (value * 100.0).round() as i32;
            return Some(pct.to_string());
        }
        None
    })
}

fn is_volume_muted(output: &str) -> bool {
    output.contains("[MUTED]")
}

fn is_battery_charging(status: &str) -> bool {
    status.trim().eq_ignore_ascii_case("charging")
}

fn parse_wpctl_device_name(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        for key in [
            "node.description",
            "device.description",
            "device.nick",
            "node.nick",
        ] {
            let marker = format!("{key} = \"");
            if let Some(start) = trimmed.find(&marker) {
                let value = &trimmed[start + marker.len()..];
                if let Some(end) = value.find('"') {
                    return Some(value[..end].to_string());
                }
            }
        }
    }
    None
}

fn format_volume_device_label(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("airpods") {
        return "Pods".to_string();
    }

    if lower.contains("speaker")
        || lower.contains("built-in")
        || lower.contains("built in")
        || lower.contains("analog stereo")
    {
        return "SPEAKER".to_string();
    }

    trimmed.chars().take(8).collect()
}

fn parse_json_i64_field(raw: &str, field: &str) -> Option<i64> {
    let marker = format!("\"{field}\":");
    let pos = raw.find(&marker)? + marker.len();
    parse_i64_from(raw, pos)
}

fn find_json_string_field(raw: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let start = raw.find(&marker)? + marker.len();
    parse_json_string(raw, start)
}

fn parse_json_string(raw: &str, start: usize) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut i = start;
    let mut out = String::new();

    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some(out),
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    return None;
                }
                match bytes[i] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let hex = raw.get(i + 1..i + 5)?;
                        let value = u16::from_str_radix(hex, 16).ok()?;
                        let ch = char::from_u32(value as u32)?;
                        out.push(ch);
                        i += 4;
                    }
                    _ => return None,
                }
            }
            byte => out.push(byte as char),
        }
        i += 1;
    }

    None
}

fn song_title_from_file(raw: &str) -> Option<String> {
    let file = find_json_string_field(raw, "file")?;
    let file_name = file.rsplit('/').next()?;
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    Some(stem.to_string())
}

fn sanitize_bar_text(input: &str) -> String {
    input
        .chars()
        .filter_map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | ' ' | '[' | ']' | ':' | '%' | '-' | '.' | '/' => {
                Some(ch)
            }
            '&' => Some(' '),
            '_' => Some(' '),
            _ => None,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_workspace_ids(raw: &str) -> Vec<i64> {
    let marker = "\"id\":";
    let mut out = Vec::new();
    let mut idx = 0;
    while let Some(off) = raw[idx..].find(marker) {
        let start = idx + off + marker.len();
        if let Some(id) = parse_i64_from(raw, start) {
            out.push(id);
        }
        idx = start;
        if idx >= raw.len() {
            break;
        }
    }
    out
}

fn parse_i64_from(s: &str, mut i: usize) -> Option<i64> {
    let b = s.as_bytes();
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    let start = i;
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    let digits = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits {
        return None;
    }
    s[start..i].parse::<i64>().ok()
}

#[derive(Deserialize)]
struct HyprMonitorRaw {
    name: String,
    #[serde(rename = "activeWorkspace")]
    active_workspace: Option<HyprWorkspaceRef>,
}

#[derive(Deserialize)]
struct HyprWorkspaceRef {
    id: i64,
}

#[derive(Deserialize)]
struct HyprWorkspaceRaw {
    id: i64,
    monitor: Option<String>,
}

struct HyprMonitorInfo {
    name: String,
    active_workspace_id: Option<i64>,
}

struct HyprWorkspaceInfo {
    id: i64,
    monitor: String,
}

fn parse_monitors(raw: &str) -> Vec<HyprMonitorInfo> {
    serde_json::from_str::<Vec<HyprMonitorRaw>>(raw)
        .unwrap_or_default()
        .into_iter()
        .map(|monitor| HyprMonitorInfo {
            name: monitor.name,
            active_workspace_id: monitor.active_workspace.map(|workspace| workspace.id),
        })
        .collect()
}

fn parse_monitor_workspaces(raw: &str) -> Vec<HyprWorkspaceInfo> {
    serde_json::from_str::<Vec<HyprWorkspaceRaw>>(raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|workspace| {
            let monitor = workspace.monitor?;
            Some(HyprWorkspaceInfo {
                id: workspace.id,
                monitor,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        find_json_string_field, format_volume_device_label, is_battery_charging, is_volume_muted,
        parse_i64_from, parse_monitor_workspaces, parse_monitors, parse_volume_percent,
        parse_workspace_ids, parse_wpctl_device_name, read_battery_metric, sanitize_bar_text,
        song_title_from_file,
    };
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn parse_i64_from_reads_positive_and_negative() {
        assert_eq!(parse_i64_from("id: 42", 3), Some(42));
        assert_eq!(parse_i64_from("val:-7", 4), Some(-7));
    }

    #[test]
    fn parse_i64_from_handles_missing_digits() {
        assert_eq!(parse_i64_from("id: x", 3), None);
        assert_eq!(parse_i64_from("id:-", 3), None);
    }

    #[test]
    fn parse_workspace_ids_collects_all_ids() {
        let raw = r#"[{"id":1},{"id":4},{"id":2}]"#;
        assert_eq!(parse_workspace_ids(raw), vec![1, 4, 2]);
    }

    #[test]
    fn parse_workspace_ids_skips_invalid_entries() {
        let raw = r#"[{"id":"bad"},{"id":9}]"#;
        assert_eq!(parse_workspace_ids(raw), vec![9]);
    }

    #[test]
    fn parse_monitors_reads_names_and_active_workspace() {
        let raw = r#"[{"name":"DP-1","activeWorkspace":{"id":2}},{"name":"HDMI-A-1","activeWorkspace":{"id":7}}]"#;
        let parsed = parse_monitors(raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "DP-1");
        assert_eq!(parsed[0].active_workspace_id, Some(2));
        assert_eq!(parsed[1].name, "HDMI-A-1");
        assert_eq!(parsed[1].active_workspace_id, Some(7));
    }

    #[test]
    fn parse_monitor_workspaces_keeps_monitor_mapping() {
        let raw = r#"[{"id":2,"monitor":"DP-1"},{"id":7,"monitor":"HDMI-A-1"}]"#;
        let parsed = parse_monitor_workspaces(raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, 2);
        assert_eq!(parsed[0].monitor, "DP-1");
        assert_eq!(parsed[1].monitor, "HDMI-A-1");
    }

    #[test]
    fn parse_volume_percent_reads_decimal_values() {
        let raw = "Volume: 0.37 [MUTED]";
        assert_eq!(parse_volume_percent(raw), Some("37".to_string()));
    }

    #[test]
    fn parse_volume_percent_returns_none_when_missing() {
        assert_eq!(parse_volume_percent("Volume: unavailable"), None);
    }

    #[test]
    fn is_volume_muted_detects_muted_flag() {
        assert!(is_volume_muted("Volume: 0.37 [MUTED]"));
        assert!(!is_volume_muted("Volume: 0.37"));
    }

    #[test]
    fn is_battery_charging_detects_status() {
        assert!(is_battery_charging("Charging\n"));
        assert!(is_battery_charging("charging"));
        assert!(!is_battery_charging("Discharging"));
        assert!(!is_battery_charging("Full"));
    }

    #[test]
    fn find_json_string_field_reads_nested_values() {
        let raw = r#"{"metadata":{"title":"Moonlight Shadow","artist":"No Hero"}}"#;
        assert_eq!(
            find_json_string_field(raw, "title"),
            Some("Moonlight Shadow".to_string())
        );
        assert_eq!(
            find_json_string_field(raw, "artist"),
            Some("No Hero".to_string())
        );
    }

    #[test]
    fn song_title_from_file_uses_basename_without_extension() {
        let raw = r#"{"file":"No Hero/no hero - moonlight shadow.mp3"}"#;
        assert_eq!(
            song_title_from_file(raw),
            Some("no hero - moonlight shadow".to_string())
        );
    }

    #[test]
    fn sanitize_bar_text_keeps_supported_glyphs() {
        assert_eq!(
            sanitize_bar_text("M83 _ Midnight City (Live)!"),
            "M83 Midnight City Live"
        );
    }

    #[test]
    fn parse_wpctl_device_name_reads_description() {
        let raw = r#"
id 48, type PipeWire:Interface:Node
    node.description = "USB Headset"
"#;
        assert_eq!(
            parse_wpctl_device_name(raw),
            Some("USB Headset".to_string())
        );
    }

    #[test]
    fn format_volume_device_label_aliases_airpods() {
        assert_eq!(format_volume_device_label("AirPods Pro"), "Pods");
    }

    #[test]
    fn format_volume_device_label_aliases_speakers() {
        assert_eq!(
            format_volume_device_label("Built-in Audio Analog Stereo"),
            "SPEAKER"
        );
    }

    #[test]
    fn format_volume_device_label_truncates_unknown_names() {
        assert_eq!(format_volume_device_label("USB Headset"), "USB Head");
    }

    #[test]
    fn read_battery_metric_tries_later_fallback_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("current_now"), "750000\n").unwrap();

        assert_eq!(
            read_battery_metric(&dir.path().to_path_buf(), &["power_now", "current_now"]),
            Some(750000.0)
        );
    }
}
