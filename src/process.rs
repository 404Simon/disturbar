use std::fs;

pub fn send_signal(sig: i32) {
    for pid in disturbar_ui_pids() {
        let _ = unsafe { libc::kill(pid, sig) };
    }
}

fn disturbar_ui_pids() -> Vec<i32> {
    let mut pids = Vec::new();
    let self_pid = std::process::id() as i32;

    let Ok(entries) = fs::read_dir("/proc") else {
        return pids;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let pid = match name.parse::<i32>() {
            Ok(pid) => pid,
            Err(_) => continue,
        };
        if pid == self_pid {
            continue;
        }

        let cmdline_path = entry.path().join("cmdline");
        let Ok(raw) = fs::read(cmdline_path) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }

        let args = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).to_string())
            .collect::<Vec<_>>();

        if !is_disturbar_ui_process(&args) {
            continue;
        }

        pids.push(pid);
    }

    pids
}

fn is_disturbar_ui_process(args: &[String]) -> bool {
    let Some(bin) = args.first() else {
        return false;
    };

    if !(bin == "disturbar" || bin.ends_with("/disturbar")) {
        return false;
    }

    !args.iter().any(|arg| arg == "--input-backend")
}
