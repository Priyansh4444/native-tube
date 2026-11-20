//! The headless launcher detaches its child. Keep that child tied to a lease
//! owned by the explorer or benchmark, even when the owner receives SIGKILL.
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::{net::UnixStream, process::CommandExt};
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

pub fn run(socket: &Path, pid_file: &Path, binary: &Path) -> io::Result<()> {
    let mut lease = UnixStream::connect(socket)?;
    lease.set_nonblocking(true)?;
    if !connected(&mut lease)? {
        return Ok(());
    }
    let parent = std::process::id();
    let mut command = Command::new(binary);
    bind_child(&mut command, libc::SIGKILL);
    let mut stamp = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: stamp points to initialized, writable memory for the syscall.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut stamp) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let mut app = command.spawn()?;
    let result = (|| {
        writeln!(lease, "{} {}.{:09}", app.id(), stamp.tv_sec, stamp.tv_nsec)?;
        while connected(&mut lease)? && app.try_wait()?.is_none() {
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    })();
    stop(&mut app);
    if fs::read_to_string(pid_file).is_ok_and(|pid| pid.trim() == parent.to_string()) {
        let _ = fs::remove_file(pid_file);
    }
    result
}

/// Arrange for the kernel to stop the direct child when its owner disappears.
pub fn bind_child(command: &mut Command, signal: libc::c_int) {
    let parent = std::process::id();
    // SAFETY: bind_process performs only async-signal-safe syscalls before exec.
    unsafe { command.pre_exec(move || bind_process(parent, signal)) };
}

pub fn bind_process(parent: u32, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: prctl and getppid need no borrowed pointers and are safe before exec.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, signal) } == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::getppid() }.cast_unsigned() != parent {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    Ok(())
}

fn connected(lease: &mut UnixStream) -> io::Result<bool> {
    match lease.read(&mut [0]) {
        Ok(0) => Ok(false),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(true),
        Err(error) => Err(error),
    }
}

fn stop(app: &mut Child) {
    if app.try_wait().is_ok_and(|status| status.is_some()) {
        return;
    }
    // SAFETY: this is our own unreaped child, so its process ID cannot have been reused.
    unsafe { libc::kill(app.id().cast_signed(), libc::SIGTERM) };
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(350) {
        if app.try_wait().is_ok_and(|status| status.is_some()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    let _ = app.kill();
    let _ = app.wait();
}
