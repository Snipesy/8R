use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use eightr_core::{input, Config};

/// 8R: deterministically undo R8 and friends, from the shipped artifact alone.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarize an input: dex files, build markers, detected sources, faithfulness summary.
    Info {
        /// A .dex, .apk/.aab/.zip, or a directory of classes*.dex.
        input: PathBuf,
    },
    /// Run the pipeline. With -o, writes classes*.dex (recovered names applied),
    /// 8r-mapping.txt (loadable by jadx), and report.json into that directory.
    Undo {
        input: PathBuf,
        /// Output directory.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Where to write the report (stdout if neither this nor -o is given).
        #[arg(long)]
        report: Option<PathBuf>,
        /// Include every label in the report.
        #[arg(long)]
        verbose: bool,
        /// Don't annotate methods with their inlining hints (`@eightr.Inlined(...)`).
        #[arg(long)]
        no_hint_annotations: bool,
    },
    /// List every registered rule.
    Rules,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("8r: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Info { input } => {
            let inputs = input::load(&input)?;
            let outcome = eightr_core::run(&inputs, &Config::default())?;
            let r = &outcome.report;
            for i in &r.inputs {
                let integrity = if i.checksum_ok && i.signature_ok { "ok" } else { "MISMATCH" };
                println!("{}  dex {:03}  {} classes  checksum {integrity}", i.name, i.dex_version, i.classes);
            }
            for m in &r.markers {
                println!(
                    "marker  {} {}  min-api {}  mode {}",
                    m.tool,
                    m.version().unwrap_or("?"),
                    m.min_api().map(|v| v.to_string()).unwrap_or("?".into()),
                    m.r8_mode().or(m.compilation_mode()).unwrap_or("?")
                );
            }
            for s in &r.sources {
                println!("source  {:?}", s.source);
                for e in &s.evidence {
                    println!("          {e}");
                }
            }
            for f in &r.findings {
                println!("{:?}: {}", f.severity, f.message);
            }
            println!("\n{:<12} {:>8} {:>8} {:>8} {:>10}", "attribute", "S", "D", "N", "untouched");
            for (attr, c) in &r.summary {
                println!(
                    "{:<12} {:>8} {:>8} {:>8} {:>10}",
                    format!("{attr:?}"),
                    c.solved,
                    c.deterministic,
                    c.nondeterministic,
                    c.untouched
                );
            }
        }
        Command::Undo { input, out, report, verbose, no_hint_annotations } => {
            let inputs = input::load(&input)?;
            let resources = input::resources(&input);
            let outcome = eightr_core::run(&inputs, &Config { verbose_labels: verbose, no_hint_annotations, resources, ..Default::default() })?;
            let json = outcome.report.to_json();
            let write = |p: &PathBuf, bytes: &[u8]| std::fs::write(p, bytes).map_err(|e| format!("{}: {e}", p.display()));
            if let Some(dir) = &out {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                let result = eightr_core::output::emit(&outcome)?;
                for (name, bytes) in &result.dex {
                    write(&dir.join(name), bytes)?;
                }
                write(&dir.join("8r-mapping.txt"), result.mapping.as_bytes())?;
                write(&dir.join("report.json"), json.as_bytes())?;
                eprintln!("wrote {} dex file(s), 8r-mapping.txt, report.json to {}", result.dex.len(), dir.display());
            }
            match report {
                Some(p) => write(&p, json.as_bytes())?,
                None if out.is_none() => print!("{json}"),
                None => {}
            }
        }
        Command::Rules => {
            for r in eightr_rules::REGISTRY {
                println!("{}  [{}]  {:?}\n    {}", r.id, r.class.letter(), r.source, r.summary);
                for p in r.preconditions {
                    println!("    - {p}");
                }
            }
        }
    }
    Ok(())
}
