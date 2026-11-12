//! Record and replay HTTP answers, so benchmarks see the same data at the same
//! speed on every run and in every frontend.
//!
//! `NT_FIXTURES=<dir>` turns it on. `NT_FIXTURE_MODE=record` fetches from the
//! network and saves every answer; `replay` (the default when the directory is
//! set) answers only from the directory and fails on anything unrecorded;
//! `auto` replays what it has and records the rest.

use crate::error::{Error, Result};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Record,
    Replay,
    Auto,
}

#[derive(Clone, Debug)]
pub struct Fixtures {
    dir: PathBuf,
    mode: Mode,
}

impl Fixtures {
    pub fn from_env() -> Option<Fixtures> {
        let dir = PathBuf::from(std::env::var_os("NT_FIXTURES")?);
        let mode = match std::env::var("NT_FIXTURE_MODE").as_deref() {
            Ok("record") => Mode::Record,
            Ok("auto") => Mode::Auto,
            _ => Mode::Replay,
        };
        let _ = std::fs::create_dir_all(&dir);
        Some(Fixtures { dir, mode })
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{:016x}.bin", fnv1a(key.as_bytes())))
    }

    /// `Ok(None)` means "go to the network".
    pub fn lookup(&self, key: &str) -> Result<Option<bytes::Bytes>> {
        if self.mode == Mode::Record {
            return Ok(None);
        }
        match std::fs::read(self.path(key)) {
            Ok(b) => Ok(Some(b.into())),
            Err(_) if self.mode == Mode::Auto => Ok(None),
            Err(_) => Err(Error::Fixture(key.chars().take(160).collect())),
        }
    }

    pub fn store(&self, key: &str, body: &[u8]) {
        if self.mode == Mode::Replay {
            return;
        }
        let path = self.path(key);
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
        // An index makes the directory auditable by hand.
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("index.txt")) {
            let _ = writeln!(f, "{:016x} {}", fnv1a(key.as_bytes()), key.chars().take(200).collect::<String>());
        }
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}
