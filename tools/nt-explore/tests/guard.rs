use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::{fs::PermissionsExt, net::UnixListener};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const EXPLORER: &str = env!("CARGO_BIN_EXE_nt-explore");
const CHILD: &str = "NT_GUARD_TEST_OWNER";

fn alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(") ")
                .map(|(_, data)| data.starts_with('Z'))
        })
        .is_some_and(|zombie| !zombie)
}

fn wait_until(check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < Duration::from_secs(4), "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn lease_owner() {
    let Some(dir) = std::env::var_os(CHILD) else {
        return;
    };
    let dir = Path::new(&dir);
    let socket = dir.join("lease");
    let marker = dir.join("bench.pid");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut guard = Command::new("setsid")
        .args([EXPLORER, "--guarded-app"])
        .arg(&socket)
        .arg(&marker)
        .arg(dir.join("dummy-app"))
        .spawn()
        .unwrap();
    fs::write(&marker, guard.id().to_string()).unwrap();
    let (socket, _) = listener.accept().unwrap();
    let mut reply = String::new();
    BufReader::new(&socket).read_line(&mut reply).unwrap();
    let app = reply.split_whitespace().next().unwrap();
    let mut input = Command::new(EXPLORER)
        .args([
            "--owned-exec",
            &std::process::id().to_string(),
            "/bin/sleep",
            "15",
        ])
        .spawn()
        .unwrap();
    fs::write(
        dir.join("pids"),
        format!("{} {app} {}", guard.id(), input.id()),
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(6));
    drop(socket);
    assert!(guard.wait().unwrap().success());
    let _ = input.kill();
    let _ = input.wait();
}

#[test]
fn sigkill_of_owner_stops_app_guard_and_input() {
    let dir = tempfile::tempdir().unwrap();
    let dummy = dir.path().join("dummy-app");
    fs::write(&dummy, "#!/bin/sh\nexec sleep 15\n").unwrap();
    fs::set_permissions(&dummy, fs::Permissions::from_mode(0o700)).unwrap();
    let mut owner = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lease_owner"])
        .env(CHILD, dir.path())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let pids = dir.path().join("pids");
    wait_until(|| fs::read_to_string(&pids).is_ok_and(|line| line.split_whitespace().count() == 3));
    let ids: Vec<u32> = fs::read_to_string(&pids)
        .unwrap()
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    assert!(ids.iter().all(|&pid| alive(pid)));
    owner.kill().unwrap();
    owner.wait().unwrap();
    wait_until(|| ids.iter().all(|&pid| !alive(pid)));
    assert!(!dir.path().join("bench.pid").exists());
}
