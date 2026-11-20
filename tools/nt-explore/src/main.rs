mod guard;
mod live;
mod model;
mod properties;

use live::{Run, Runner};
use model::Trace;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Exit codes: 0 all properties held, 1 a property failed (or a shrink finished), 3 the app
/// printed no NT_STATE, 4 another GUI test holds the desktop (nothing ran).
const USAGE: &str = "nt-explore <app binary> [--seed SEED] [--steps N] [--minutes N]\nnt-explore [<app binary>] --replay <trace.json>\nnt-explore [<app binary>] --shrink <trace.json>";

/// Replays one shrink may run, each a GUI session of at most two minutes.
const MAX_SHRINK_TRIALS: usize = 12;

enum Mode {
    Explore,
    Replay(PathBuf),
    Shrink(PathBuf),
}

struct Options {
    app: Option<PathBuf>,
    seed: u64,
    steps: Option<usize>,
    minutes: Option<f64>,
    mode: Mode,
}

fn parse() -> io::Result<Options> {
    let mut options = Options {
        app: None,
        seed: rand::random(),
        steps: None,
        minutes: None,
        mode: Mode::Explore,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" | "--steps" | "--minutes" | "--replay" | "--shrink" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::other(format!("missing value for {arg}")))?;
                match arg.as_str() {
                    "--seed" => options.seed = value.parse().map_err(io::Error::other)?,
                    "--steps" => options.steps = Some(value.parse().map_err(io::Error::other)?),
                    "--minutes" => {
                        let minutes = value.parse::<f64>().map_err(io::Error::other)?;
                        if !minutes.is_finite() || !(0.0 < minutes && minutes <= 2.0) {
                            return Err(io::Error::other(
                                "--minutes must be greater than zero and at most 2",
                            ));
                        }
                        options.minutes = Some(minutes);
                    }
                    "--replay" | "--shrink" if matches!(options.mode, Mode::Explore) => {
                        options.mode = if arg == "--replay" {
                            Mode::Replay(value.into())
                        } else {
                            Mode::Shrink(value.into())
                        };
                    }
                    _ => {
                        return Err(io::Error::other(
                            "only one replay or shrink mode is allowed",
                        ));
                    }
                }
            }
            arg if !arg.starts_with('-') && options.app.is_none() => options.app = Some(arg.into()),
            _ => return Err(io::Error::other(format!("unexpected argument {arg}"))),
        }
    }
    Ok(options)
}

fn result(run: &Run, out: &Path) -> ExitCode {
    let actions = run.trace.actions().len();
    println!(
        "seed={} actions={} trace={}",
        run.trace.seed,
        actions,
        out.join("trace.json").display()
    );
    if let Some(violation) = &run.trace.violation {
        eprintln!("{:?}: {}", violation.property, violation.detail);
        if let Some(screenshot) = &violation.screenshot {
            eprintln!("screenshot {screenshot}");
        }
        return ExitCode::from(if run.missing_probe { 3 } else { 1 });
    }
    println!("all observed properties passed");
    ExitCode::SUCCESS
}

fn run() -> io::Result<ExitCode> {
    let options = parse()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let out = root.join("tools/nt-explore/out");
    fs::create_dir_all(&out)?;
    let bundle = tempfile::Builder::new()
        .prefix("run-")
        .tempdir_in(out)?
        .keep();
    let runner = Runner::new(root, bundle.clone());
    let shrinking = matches!(options.mode, Mode::Shrink(_));
    match options.mode {
        Mode::Explore => {
            let app = options
                .app
                .ok_or_else(|| io::Error::other("supply an app binary"))?;
            let steps = options.steps.unwrap_or(if options.minutes.is_some() {
                usize::MAX
            } else {
                100
            });
            let run = runner.run(&app, options.seed, steps, options.minutes, None, "explore")?;
            Ok(result(&run, &bundle.join("explore")))
        }
        Mode::Replay(trace) | Mode::Shrink(trace) => {
            let trace: Trace = serde_json::from_slice(&fs::read(trace)?)?;
            if trace.version != 1 {
                return Err(io::Error::other(format!(
                    "unsupported trace version {}",
                    trace.version
                )));
            }
            let app = options.app.unwrap_or_else(|| PathBuf::from(&trace.app));
            let actions = trace.actions();
            if !shrinking {
                let run = runner.run(
                    &app,
                    trace.seed,
                    actions.len(),
                    None,
                    Some(&actions),
                    "replay",
                )?;
                return Ok(result(&run, &bundle.join("replay")));
            }
            let expected = trace
                .violation
                .ok_or_else(|| io::Error::other("trace has no violation"))?
                .property;
            let mut trial = 0;
            let mut last_match = String::new();
            let minimized = minimize(actions, |candidate| {
                // Every trial is a GUI session; stop before shrinking turns into a long one.
                if trial >= MAX_SHRINK_TRIALS {
                    return Ok(false);
                }
                if trial > 0 {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
                trial += 1;
                let name = format!("trial-{trial}");
                let run = runner.run(
                    &app,
                    trace.seed,
                    candidate.len(),
                    None,
                    Some(candidate),
                    &name,
                )?;
                let matched = !run.missing_probe
                    && run
                        .trace
                        .violation
                        .as_ref()
                        .is_some_and(|found| found.property == expected);
                println!(
                    "trial={trial} actions={} matched={matched}",
                    candidate.len()
                );
                if matched {
                    last_match = name;
                }
                Ok(matched)
            })?;
            let path = bundle.join(last_match).join("trace.json");
            println!(
                "retained {expected:?} with {} actions: {}",
                minimized.len(),
                path.display()
            );
            // A successfully shrunk counterexample is still a failed property.
            Ok(ExitCode::FAILURE)
        }
    }
}

fn minimize<T: Clone>(
    mut sequence: Vec<T>,
    mut confirms: impl FnMut(&[T]) -> io::Result<bool>,
) -> io::Result<Vec<T>> {
    if !confirms(&sequence)? {
        return Err(io::Error::other(
            "original trace did not reproduce the same property",
        ));
    }
    let mut parts = 2;
    while !sequence.is_empty() {
        let chunk = sequence.len().div_ceil(parts);
        let mut reduced = false;
        for start in (0..sequence.len()).step_by(chunk) {
            let end = (start + chunk).min(sequence.len());
            let trial = [&sequence[..start], &sequence[end..]].concat();
            if confirms(&trial)? {
                sequence = trial;
                parts = parts.saturating_sub(1).max(2);
                reduced = true;
                break;
            }
        }
        if !reduced {
            if parts >= sequence.len() {
                break;
            }
            parts = (parts * 2).min(sequence.len());
        }
    }
    Ok(sequence)
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let [flag, socket, pid_file, binary] = args.as_slice()
        && flag == "--guarded-app"
    {
        return match guard::run(Path::new(socket), Path::new(pid_file), Path::new(binary)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("nt-explore guard: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if let [flag, owner, program, rest @ ..] = args.as_slice()
        && flag == "--owned-exec"
    {
        let error = owner.to_str().and_then(|pid| pid.parse().ok()).map_or_else(
            || io::Error::other("invalid owner PID"),
            |pid| match guard::bind_process(pid, libc::SIGTERM) {
                Ok(()) => std::process::Command::new(program).args(rest).exec(),
                Err(error) => error,
            },
        );
        eprintln!("nt-explore exec: {error}");
        return ExitCode::FAILURE;
    }
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run() {
        Ok(status) => status,
        // The desktop is busy with another GUI test: nothing was driven, try again later.
        Err(error) if error.kind() == io::ErrorKind::ResourceBusy => {
            eprintln!("nt-explore: {error}");
            ExitCode::from(4)
        }
        Err(error) => {
            eprintln!("nt-explore: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrink_preserves_order_and_removes_irrelevant_actions() {
        let trace = vec!["scroll", "search", "escape", "open", "pause", "back"];
        let shrunk = minimize(trace, |actions| {
            Ok(actions.contains(&"search") && actions.contains(&"back"))
        })
        .unwrap();
        assert_eq!(shrunk, ["search", "back"]);
    }

    #[test]
    fn shrink_accepts_a_startup_violation_without_actions() {
        let shrunk = minimize(vec!["escape"], |_| Ok(true)).unwrap();
        assert!(shrunk.is_empty());
    }

    #[test]
    fn shrink_rejects_nonreproducing_trace() {
        assert!(minimize(vec!["back"], |_| Ok(false)).is_err());
    }
}
