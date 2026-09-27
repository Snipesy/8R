use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use eightr_core::libdb::{Coord, Profile};

/// Forge LibDB packs: library fingerprints from scenario builds of an app's build profile.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print an app's build profile (JSON) from its APK/dex, without running 8R.
    Profile { input: PathBuf },
    /// Resolve the library closure of a profile and print the lock.
    Resolve {
        /// A profile JSON file, or an APK/dex to read it from.
        profile: PathBuf,
        /// Pin a module version (`group:artifact:version`), overriding the profile.
        #[arg(long)]
        pin: Vec<String>,
    },
    /// Build (or reuse from the cache) the pack of a profile; prints its path.
    Build {
        /// A profile JSON file, or an APK/dex to read it from.
        profile: PathBuf,
        /// Scenario catalog (default: the built-in one).
        #[arg(long)]
        scenarios: Option<PathBuf>,
        #[arg(long)]
        pin: Vec<String>,
        /// Parallel scenario builds.
        #[arg(long, default_value_t = 2)]
        jobs: usize,
        /// Also copy the pack here.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Summarize a pack.
    Show { pack: PathBuf },
    /// Developer check: exact matching of a mapped app (dex directory + mapping) against a pack.
    Grade { pack: PathBuf, app: PathBuf, mapping: PathBuf },
}

fn load_profile(p: &PathBuf) -> Result<Profile, String> {
    if p.extension().is_some_and(|x| x == "json") {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        return Profile::parse(&text);
    }
    let inputs = eightr_core::input::load(p).map_err(|e| e.to_string())?;
    let resources = if p.is_dir() { eightr_core::input::dir_resources(p) } else { eightr_core::input::resources(p) };
    Profile::from_inputs(&inputs, &resources)?.ok_or_else(|| format!("{}: no R8 marker, so no build profile", p.display()))
}

fn pins(v: &[String]) -> Result<Vec<Coord>, String> {
    v.iter().map(|s| Coord::parse(s).ok_or_else(|| format!("bad --pin {s} (group:artifact:version)"))).collect()
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Profile { input } => {
            let p = load_profile(&input)?;
            println!("{}", serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?);
        }
        Command::Resolve { profile, pin } => {
            for l in eightr_forge::build::lock(&load_profile(&profile)?, &pins(&pin)?)? {
                println!("{l}");
            }
        }
        Command::Build { profile, scenarios, pin, jobs, out } => {
            let catalog = match scenarios {
                Some(p) => std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?,
                None => eightr_forge::catalog::DEFAULT.to_string(),
            };
            let opts = eightr_forge::build::Options { catalog, pins: pins(&pin)?, jobs, log: true };
            let (path, pack) = eightr_forge::build::build(&load_profile(&profile)?, &opts)?;
            eprintln!("pack: {} records, {} methods, {} scenarios", pack.records.len(), pack.methods.len(), pack.scenarios.len());
            if let Some(o) = out {
                std::fs::copy(&path, &o).map_err(|e| format!("{}: {e}", o.display()))?;
            }
            println!("{}", path.display());
        }
        Command::Grade { pack, app, mapping } => {
            let p = eightr_core::libdb::Pack::decode(&std::fs::read(&pack).map_err(|e| format!("{}: {e}", pack.display()))?)?;
            let g = eightr_forge::grade::grade(&p, &app, &mapping)?;
            let pct = |a: usize, b: usize| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
            println!("library methods {}; matched {} ({:.1}% recall), correct {} ({:.2}% precision)", g.library_methods, g.matched, pct(g.correct, g.library_methods), g.correct, pct(g.correct, g.matched));
            println!("universe-unique matches {}, correct {} ({:.2}%)", g.unique_matched, g.unique_correct, pct(g.unique_correct, g.unique_matched));
            println!("inline frames: {} correct matches have inlined code; the pack's frame table equals the app's own for {} ({:.1}%)", g.with_frames, g.frames_equal, pct(g.frames_equal, g.with_frames));
            for w in &g.wrong {
                println!("  wrong: {w}");
            }
        }
        Command::Show { pack } => {
            let p = eightr_core::libdb::Pack::decode(&std::fs::read(&pack).map_err(|e| format!("{}: {e}", pack.display()))?)?;
            println!("profile {}  ({})", p.profile.key(), p.profile.canonical());
            for (k, v) in &p.tools {
                println!("tool {k}: {v}");
            }
            println!("{} artifacts, {} scenarios, {} classes, {} methods, {} records ({} informative, {} unique), {} stacks", p.artifacts.len(), p.scenarios.len(), p.classes.len(), p.methods.len(), p.records.len(), p.records.iter().filter(|r| r.informative).count(), p.records.iter().filter(|r| r.informative && r.unique).count(), p.stacks.len());
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("8r-forge: {e}");
            ExitCode::FAILURE
        }
    }
}
