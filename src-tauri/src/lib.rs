use serde::Serialize;
use tauri::Manager;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::Command;
use std::{
    collections::{HashMap, HashSet},
    fs,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const AGENT: &str = "agent-browser-linux-x64";
const AGENT_PROFILE: &str = "/tmp/agent-browser-chrome-";

#[derive(Clone, Serialize)]
struct ProcessInfo {
    pid: i32,
    ppid: i32,
    name: String,
    state: String,
    cpu_percent: f64,
    memory_mib: f64,
    elapsed_seconds: u64,
    command: String,
    #[serde(skip_serializing)]
    marker: u64,
}
#[derive(Serialize)]
struct Session {
    id: String,
    kind: String,
    profile: Option<String>,
    root_pid: i32,
    agent_pid: Option<i32>,
    processes: Vec<ProcessInfo>,
    cpu_percent: f64,
    memory_mib: f64,
    active_seconds: u64,
}
#[derive(Serialize)]
struct Snapshot {
    sessions: Vec<Session>,
    agent_process_count: usize,
    chrome_process_count: usize,
    cpu_busy_percent: f64,
    memory_available_mib: u64,
    swap_used_mib: u64,
    platform: String,
    captured_at: u64,
}
struct AppState {
    cpu: Mutex<Option<(u64, u64, Instant)>>,
    samples: Mutex<HashMap<i32, (u64, u64, Instant)>>,
}

fn redact(s: &str) -> String {
    s.split_whitespace()
        .map(|x| {
            let l = x.to_lowercase();
            if x.starts_with("http://") || x.starts_with("https://") {
                "[url]".into()
            } else if l.contains("token=") || l.contains("key=") || l.contains("password=") {
                "[redacted-argument]".into()
            } else {
                x.into()
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}
fn profile(s: &str) -> Option<String> {
    let p = s.find("--user-data-dir=")? + 16;
    let v = s[p..]
        .split_whitespace()
        .next()?
        .trim_matches('"')
        .to_string();
    (v.contains(AGENT_PROFILE) || v.to_lowercase().contains("playwright")).then_some(v)
}
fn agent(p: &ProcessInfo) -> bool {
    p.command.contains(AGENT)
}
fn playwright(p: &ProcessInfo) -> bool {
    let c = p.command.to_lowercase();
    c.contains("playwright-core") || c.contains("@playwright/test") || c.contains("playwright/cli")
}
fn browser(p: &ProcessInfo) -> bool {
    let n = p.name.to_lowercase();
    n == "chrome"
        || n == "chromium"
        || n.contains("firefox")
        || n.contains("webkit")
        || n.contains("msedge")
}

fn descendants(root: i32, all: &HashMap<i32, ProcessInfo>) -> HashSet<i32> {
    let mut kids: HashMap<i32, Vec<i32>> = HashMap::new();
    for p in all.values() {
        kids.entry(p.ppid).or_default().push(p.pid)
    }
    let mut seen = HashSet::from([root]);
    let mut q = vec![root];
    while let Some(x) = q.pop() {
        for y in kids.get(&x).into_iter().flatten() {
            if seen.insert(*y) {
                q.push(*y)
            }
        }
    }
    seen
}
fn make_session(
    id: String,
    profile: Option<String>,
    root: i32,
    agent_pid: Option<i32>,
    ids: HashSet<i32>,
    all: &HashMap<i32, ProcessInfo>,
) -> Session {
    let mut processes: Vec<_> = ids
        .into_iter()
        .filter_map(|p| all.get(&p).cloned())
        .collect();
    processes.sort_by_key(|p| p.pid);
    let kind = if profile.as_ref().is_some_and(|p| p.contains(AGENT_PROFILE))
        || processes.iter().any(agent)
    {
        "Agent browser"
    } else {
        "Playwright"
    };
    let cpu_percent = processes.iter().map(|p| p.cpu_percent).sum();
    let memory_mib = processes.iter().map(|p| p.memory_mib).sum();
    let active_seconds = all
        .get(&root)
        .map(|process| process.elapsed_seconds)
        .unwrap_or_else(|| processes.iter().map(|process| process.elapsed_seconds).min().unwrap_or(0));
    Session {
        id,
        kind: kind.into(),
        profile,
        root_pid: root,
        agent_pid,
        processes,
        cpu_percent,
        memory_mib,
        active_seconds,
    }
}
fn sessions(all: &HashMap<i32, ProcessInfo>) -> Vec<Session> {
    let roots: Vec<_> = all
        .values()
        .filter(|p| agent(p) || playwright(p))
        .map(|p| p.pid)
        .collect();
    let mut out = vec![];
    let mut claimed: HashSet<i32> = HashSet::new();
    for pid in roots {
        let ids = descendants(pid, all);
        let prof = ids
            .iter()
            .filter_map(|id| all.get(id))
            .find_map(|p| profile(&p.command));
        claimed.extend(ids.iter());
        let is_agent = all.get(&pid).is_some_and(agent);
        out.push(make_session(
            format!("{}:{pid}", if is_agent { "agent" } else { "playwright" }),
            prof,
            pid,
            is_agent.then_some(pid),
            ids,
            all,
        ));
    }
    let mut grouped: HashMap<String, HashSet<i32>> = HashMap::new();
    for p in all.values() {
        if !claimed.contains(&p.pid) {
            if let Some(v) = profile(&p.command) {
                grouped.entry(v).or_default().insert(p.pid);
            }
        }
    }
    for (prof, mut ids) in grouped {
        let mut root = *ids.iter().next().unwrap();
        for id in ids.clone() {
            let mut a = id;
            while let Some(parent) = all.get(&a).and_then(|p| all.get(&p.ppid)) {
                if !browser(parent) {
                    break;
                }
                a = parent.pid
            }
            root = root.min(a);
            ids.extend(descendants(a, all));
        }
        out.push(make_session(
            format!("profile:{prof}"),
            Some(prof),
            root,
            None,
            ids,
            all,
        ));
    }
    out.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
    out
}

#[cfg(target_os = "linux")]
fn linux_num(line: Option<&str>) -> u64 {
    line.and_then(|x| x.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or(0)
}
#[cfg(target_os = "linux")]
fn process_map(state: &AppState) -> HashMap<i32, ProcessInfo> {
    let up = fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|x| x.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.);
    let mut out = HashMap::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for e in entries.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            let b = format!("/proc/{pid}");
            let Ok(stat) = fs::read_to_string(format!("{b}/stat")) else {
                continue;
            };
            let Some(i) = stat.rfind(')') else { continue };
            let f: Vec<_> = stat[i + 2..].split_whitespace().collect();
            let (Ok(ppid), Ok(u), Ok(s), Ok(marker)) = (
                f.get(1).unwrap_or(&"").parse(),
                f.get(11).unwrap_or(&"").parse::<u64>(),
                f.get(12).unwrap_or(&"").parse::<u64>(),
                f.get(19).unwrap_or(&"").parse::<u64>(),
            ) else {
                continue;
            };
            let now = Instant::now();
            let ticks = u + s;
            let cpu = state
                .samples
                .lock()
                .ok()
                .and_then(|mut m| {
                    m.insert(pid, (ticks, marker, now))
                        .and_then(|(old, oldmark, t)| {
                            if oldmark == marker && now.duration_since(t).as_secs_f64() > 0. {
                                Some(
                                    (ticks.saturating_sub(old) as f64 / 100.)
                                        / now.duration_since(t).as_secs_f64()
                                        * 100.,
                                )
                            } else {
                                None
                            }
                        })
                })
                .unwrap_or(0.);
            let cmd = fs::read(format!("{b}/cmdline"))
                .ok()
                .map(|x| {
                    String::from_utf8_lossy(&x)
                        .replace('\0', " ")
                        .trim()
                        .to_string()
                })
                .filter(|x| !x.is_empty())
                .unwrap_or_else(|| fs::read_to_string(format!("{b}/comm")).unwrap_or_default());
            let status = fs::read_to_string(format!("{b}/status")).unwrap_or_default();
            let rss = status
                .lines()
                .find(|x| x.starts_with("VmRSS:"))
                .map(|x| linux_num(Some(x)))
                .unwrap_or(0);
            out.insert(
                pid,
                ProcessInfo {
                    pid,
                    ppid,
                    name: fs::read_to_string(format!("{b}/comm"))
                        .unwrap_or_default()
                        .trim()
                        .into(),
                    state: f[0].into(),
                    cpu_percent: cpu,
                    memory_mib: rss as f64 / 1024.,
                    elapsed_seconds: (up - marker as f64 / 100.).max(0.) as u64,
                    command: redact(&cmd),
                    marker,
                },
            );
        }
    }
    out
}
#[cfg(target_os = "macos")]
fn process_map(_: &AppState) -> HashMap<i32, ProcessInfo> {
    let o = Command::new("ps")
        .args(["-axo", "pid=,ppid=,state=,etime=,rss=,pcpu=,comm=,args="])
        .output()
        .ok();
    String::from_utf8_lossy(&o.map(|x| x.stdout).unwrap_or_default())
        .lines()
        .filter_map(|l| {
            let f: Vec<_> = l
                .splitn(8, char::is_whitespace)
                .filter(|x| !x.is_empty())
                .collect();
            let (pid, ppid) = (f.first()?.parse().ok()?, f.get(1)?.parse().ok()?);
            let age = f[3]
                .split(&['-', ':'][..])
                .filter_map(|x| x.parse::<u64>().ok())
                .fold(0, |a, x| a * 60 + x);
            Some((
                pid,
                ProcessInfo {
                    pid,
                    ppid,
                    name: (*f.get(6)?).to_string(),
                    state: (*f.get(2)?).to_string(),
                    elapsed_seconds: age,
                    memory_mib: f.get(4)?.parse::<f64>().ok()? / 1024.,
                    cpu_percent: f.get(5)?.parse().unwrap_or(0.),
                    command: redact(f.get(7).unwrap_or(f.get(6)?)),
                    marker: 0,
                },
            ))
        })
        .collect()
}
#[cfg(target_os = "windows")]
fn process_map(_: &AppState) -> HashMap<i32, ProcessInfo> {
    let script="Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,CommandLine,WorkingSetSize | ConvertTo-Json -Compress";
    let o = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .ok();
    let v: serde_json::Value = o
        .and_then(|x| serde_json::from_slice(&x.stdout).ok())
        .unwrap_or_default();
    let rows = v
        .as_array()
        .cloned()
        .unwrap_or_else(|| v.is_object().then_some(vec![v]).unwrap_or_default());
    rows.into_iter()
        .filter_map(|v| {
            let pid = v.get("ProcessId")?.as_i64()? as i32;
            let ppid = v
                .get("ParentProcessId")
                .and_then(|x| x.as_i64())
                .unwrap_or(0) as i32;
            let name = v
                .get("Name")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let cmd = v
                .get("CommandLine")
                .and_then(|x| x.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| name.clone());
            Some((
                pid,
                ProcessInfo {
                    pid,
                    ppid,
                    name,
                    state: "Running".into(),
                    cpu_percent: 0.,
                    memory_mib: v
                        .get("WorkingSetSize")
                        .and_then(|x| x.as_f64())
                        .unwrap_or(0.)
                        / 1048576.,
                    elapsed_seconds: 0,
                    command: redact(&cmd),
                    marker: 0,
                },
            ))
        })
        .collect()
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn process_map(_: &AppState) -> HashMap<i32, ProcessInfo> {
    HashMap::new()
}

#[cfg(target_os = "linux")]
fn system_stats(state: &AppState) -> (f64, u64, u64) {
    let s = fs::read_to_string("/proc/stat").unwrap_or_default();
    let v: Vec<u64> = s
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    let total: u64 = v.iter().sum();
    let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
    let now = Instant::now();
    let cpu = state
        .cpu
        .lock()
        .ok()
        .and_then(|mut x| {
            x.replace((total, idle, now)).map(|(ot, oi, t)| {
                let d = total.saturating_sub(ot);
                if d == 0 || now.duration_since(t) < Duration::from_millis(100) {
                    0.
                } else {
                    (1. - idle.saturating_sub(oi) as f64 / d as f64) * 100.
                }
            })
        })
        .unwrap_or(0.);
    let m = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let avail = m
        .lines()
        .find(|x| x.starts_with("MemAvailable:"))
        .map(|x| linux_num(Some(x)))
        .unwrap_or(0)
        / 1024;
    let st = m
        .lines()
        .find(|x| x.starts_with("SwapTotal:"))
        .map(|x| linux_num(Some(x)))
        .unwrap_or(0);
    let sf = m
        .lines()
        .find(|x| x.starts_with("SwapFree:"))
        .map(|x| linux_num(Some(x)))
        .unwrap_or(0);
    (cpu, avail, st.saturating_sub(sf) / 1024)
}
#[cfg(not(target_os = "linux"))]
fn system_stats(_: &AppState) -> (f64, u64, u64) {
    (0.0, 0, 0)
}

#[tauri::command]
fn get_snapshot(state: tauri::State<'_, AppState>) -> Snapshot {
    let p = process_map(&state);
    let (a, c) = (
        p.values().filter(|x| agent(x)).count(),
        p.values().filter(|x| profile(&x.command).is_some()).count(),
    );
    let (cpu, mem, swap) = system_stats(&state);
    Snapshot {
        sessions: sessions(&p),
        agent_process_count: a,
        chrome_process_count: c,
        cpu_busy_percent: cpu,
        memory_available_mib: mem,
        swap_used_mib: swap,
        platform: std::env::consts::OS.into(),
        captured_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    }
}
#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}
#[cfg(unix)]
fn stop(pid: i32, force: bool) {
    unsafe {
        kill(pid, if force { 9 } else { 15 });
    }
}
#[cfg(target_os = "windows")]
fn stop(pid: i32, _: bool) {
    let _ = Command::new("taskkill.exe")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}
#[tauri::command]
fn terminate_session(session_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let p = process_map(&state);
    let mut targets = sessions(&p)
        .into_iter()
        .find(|x| x.id == session_id)
        .ok_or("Session is no longer a verified agent-browser or Playwright tree.")?
        .processes;
    targets.sort_by_key(|x| std::cmp::Reverse(x.pid));
    #[cfg(unix)]
    {
        for x in &targets {
            stop(x.pid, false)
        }
        std::thread::sleep(Duration::from_millis(700));
        let now = process_map(&state);
        for x in &targets {
            if now.get(&x.pid).is_some_and(|y| y.marker == x.marker) {
                stop(x.pid, true)
            }
        }
    }
    #[cfg(target_os = "windows")]
    for x in &targets {
        stop(x.pid, true)
    }
    Ok(())
}
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            cpu: Mutex::new(None),
            samples: Mutex::new(HashMap::new()),
        })
        .setup(|app| {
            let window = app.get_webview_window("main").expect("main window");
            window.set_icon(tauri::include_image!("icons/128x128.png"))?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_snapshot, terminate_session])
        .run(tauri::generate_context!())
        .expect("error while running Agent Browser Monitor");
}
