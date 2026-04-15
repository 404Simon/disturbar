use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub struct BarStatus {
    pub workspaces: String,
    pub battery: String,
    pub volume: String,
    pub datetime: String,
}

impl BarStatus {
    pub fn gather() -> Self {
        Self {
            workspaces: format_workspaces(),
            battery: format_battery(),
            volume: format_volume(),
            datetime: format_datetime(),
        }
    }
}

fn format_workspaces() -> String {
    let active_raw = run_hyprctl(&["-j", "activeworkspace"]).unwrap_or_default();
    let list_raw = run_hyprctl(&["-j", "workspaces"]).unwrap_or_default();

    let active_id = parse_json_i64_field(&active_raw, "id").unwrap_or(1);
    let mut ids = parse_workspace_ids(&list_raw);

    ids.sort_unstable();
    ids.dedup();

    if ids.is_empty() {
        return "[1] 2 3 4 5".to_string();
    }

    ids.into_iter()
        .map(|id| {
            if id == active_id {
                format!("[{id}]")
            } else {
                id.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_battery() -> String {
    let (value, charging) = read_battery_state().unwrap_or_else(|| ("--".to_string(), false));
    if charging {
        format!("BAT CH {value}%")
    } else {
        format!("BAT {value}%")
    }
}

fn format_volume() -> String {
    let raw = run_cmd("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).unwrap_or_default();
    let value = parse_volume_percent(&raw).unwrap_or_else(|| "--".to_string());
    if is_volume_muted(&raw) {
        format!("VOL MUT {value}%")
    } else {
        format!("VOL {value}%")
    }
}

fn format_datetime() -> String {
    run_cmd("date", &["+%a %d %b %H:%M"])
        .map(|s| s.trim().to_ascii_uppercase())
        .unwrap_or_else(|| "-- -- --- --:--".to_string())
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

fn read_battery_state() -> Option<(String, bool)> {
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

        return Some((capacity, charging));
    }
    None
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

fn parse_json_i64_field(raw: &str, field: &str) -> Option<i64> {
    let marker = format!("\"{field}\":");
    let pos = raw.find(&marker)? + marker.len();
    parse_i64_from(raw, pos)
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

#[cfg(test)]
mod tests {
    use super::{
        is_battery_charging, is_volume_muted, parse_i64_from, parse_volume_percent,
        parse_workspace_ids,
    };

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
}
