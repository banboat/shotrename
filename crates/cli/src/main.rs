mod apply;
mod locations;
mod planning;
mod ui;
mod watch;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use planning::{build_plan, collect_files, load_template, parse_tz, print_plan};
use shotrename_core::Plan;
use shotrename_meta::{probe, ProbeOptions};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "shotrename",
    version,
    about = "Batch rename camera footage by metadata and sidecars"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List supported video files and their metadata.
    Scan(ScanArgs),
    /// Build a rename plan without touching the filesystem.
    Plan(PlanArgs),
    /// Apply a plan and write an undo journal.
    Apply(ApplyArgs),
    /// Undo the most recent batch in a journal.
    Undo(UndoArgs),
    /// Watch a folder and rename files as they arrive.
    Watch(watch::WatchArgs),
    /// Open a local web UI.
    Ui(ui::UiArgs),
    /// List built-in templates and template fields.
    Templates,
}

#[derive(Debug, Parser)]
struct ScanArgs {
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Print JSON instead of a table.
    #[arg(long)]
    json: bool,
    /// UTC offset used for QuickTime timestamps, e.g. +08:00 or 8.
    #[arg(long)]
    tz: Option<String>,
}

#[derive(Debug, Parser)]
struct PlanArgs {
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Built-in template name (dji, insta360, gopro) or a path to a TOML file.
    #[arg(long, default_value = "dji")]
    template: String,
    /// Write the plan JSON to this file.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Organise under this directory instead of each input root.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Rename files even when they already look organised.
    #[arg(long)]
    force: bool,
    /// Print JSON instead of a table.
    #[arg(long)]
    json: bool,
    #[arg(long)]
    tz: Option<String>,
}

#[derive(Debug, Parser)]
struct ApplyArgs {
    /// Plan JSON produced by `shotrename plan --out`.
    plan: PathBuf,
    /// Journal path. Defaults to .shotrename/journal.jsonl in the current directory.
    #[arg(long)]
    journal: Option<PathBuf>,
    /// Validate and print operations without moving anything.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Parser)]
struct UndoArgs {
    #[arg(long)]
    journal: Option<PathBuf>,
    /// Only verify and print the reverse operations.
    #[arg(long)]
    dry_run: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Scan(args) => scan(args),
        Command::Plan(args) => plan(args),
        Command::Apply(args) => apply(args),
        Command::Undo(args) => undo(args),
        Command::Watch(args) => watch::run(args),
        Command::Ui(args) => ui::run(args),
        Command::Templates => templates(),
    }
}

fn scan(args: ScanArgs) -> Result<()> {
    let files = collect_files(&args.paths)?;
    let options = ProbeOptions {
        tz_offset_secs: args.tz.as_deref().map(parse_tz).transpose()?,
    };
    let mut probes = Vec::new();
    for (file, _root) in files {
        match probe(&file, &options) {
            Ok(probe) => probes.push(probe),
            Err(error) => eprintln!("skip {}: {error:#}", file.display()),
        }
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&probes)?);
        return Ok(());
    }
    println!(
        "{:<44} {:<10} {:<22} {:<20} {:<16} {:<10} {:<9}",
        "FILE", "FAMILY", "CAMERA", "CAPTURED", "LOCATION", "RESOLUTION", "TIME"
    );
    for probe in &probes {
        println!(
            "{:<44} {:<10} {:<22} {:<20} {:<16} {:<10} {:<9}",
            truncate(
                &probe
                    .path
                    .file_name()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default(),
                44
            ),
            probe.family.label(),
            truncate(&probe.camera.clone().unwrap_or_default(), 22),
            probe
                .captured
                .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default(),
            truncate(
                &probe
                    .place
                    .as_ref()
                    .map(|place| place.name.clone())
                    .unwrap_or_default(),
                16
            ),
            probe.resolution(),
            probe.time_source.label()
        );
    }
    println!("\n{} file(s)", probes.len());
    Ok(())
}

fn plan(args: PlanArgs) -> Result<()> {
    let template = load_template(&args.template)?;
    let files = collect_files(&args.paths)?;
    let options = ProbeOptions {
        tz_offset_secs: args.tz.as_deref().map(parse_tz).transpose()?,
    };
    let plan = build_plan(
        &files,
        &template,
        &options,
        args.output.as_deref(),
        args.force,
    )?;

    if let Some(out) = &args.out {
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(out, serde_json::to_string_pretty(&plan)?)
            .with_context(|| format!("write {}", out.display()))?;
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        print_plan(&plan);
    }
    Ok(())
}

fn apply(args: ApplyArgs) -> Result<()> {
    let plan: Plan = serde_json::from_str(
        &fs::read_to_string(&args.plan).with_context(|| format!("read {}", args.plan.display()))?,
    )
    .context("parse plan")?;
    let journal = args
        .journal
        .unwrap_or_else(|| PathBuf::from(".shotrename/journal.jsonl"));
    let report = apply::apply_plan(&plan, &journal, args.dry_run)?;
    if !args.dry_run {
        if report.moved == 0 {
            println!("nothing to move");
        } else {
            println!(
                "applied {} file(s); journal: {}",
                report.moved,
                journal.display()
            );
        }
    }
    Ok(())
}

fn undo(args: UndoArgs) -> Result<()> {
    let journal = args
        .journal
        .unwrap_or_else(|| PathBuf::from(".shotrename/journal.jsonl"));
    let report = apply::undo_last(&journal, args.dry_run)?;
    if !args.dry_run {
        println!(
            "undid batch {} ({} file(s))",
            report.batch_id, report.restored
        );
    }
    Ok(())
}

fn templates() -> Result<()> {
    for (name, body) in [
        ("dji", shotrename_core::BUILTIN_DJI),
        ("insta360", shotrename_core::BUILTIN_INSTA360),
        ("gopro", shotrename_core::BUILTIN_GOPRO),
    ] {
        let template = shotrename_core::Template::from_toml(body)?;
        println!("{name:<10} {}", template.filename);
    }
    println!(
        "\nfields: date, time, camera, lens, location, location_zh, country, gps,\n        \
         resolution, fps, duration, codec, original, family, width, height,\n        \
         altitude, seq"
    );
    Ok(())
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_offsets() {
        assert_eq!(parse_tz("8").unwrap(), 8 * 3600);
        assert_eq!(parse_tz("+08:00").unwrap(), 8 * 3600);
        assert_eq!(parse_tz("-05:30").unwrap(), -(5 * 3600 + 30 * 60));
        assert!(parse_tz("nope").is_err());
    }
}
