use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use shotrename_core::{
    builtin_template, sanitize_or_renamed, sanitize_relative_dir, JournalEntry, NameAllocator,
    Operation, Plan, PlanEntry, PlanStatus, RelatedMode, RelatedMove, Template,
};
use shotrename_meta::{
    is_excluded_dir, is_supported_video, is_temporary, probe, related_files, MediaProbe,
    ProbeOptions,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

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
    /// List built-in templates.
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
        "{:<48} {:<10} {:<20} {:<24} {:<10} {:<8}",
        "FILE", "CAMERA", "CAPTURED", "MODEL", "RESOLUTION", "TIME"
    );
    for probe in &probes {
        println!(
            "{:<48} {:<10} {:<20} {:<24} {:<10} {:<8}",
            probe
                .path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default(),
            probe.family.label(),
            probe
                .captured
                .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default(),
            probe.camera.clone().unwrap_or_default(),
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
    let plan = build_plan(&files, &template, &options, args.output.as_deref())?;

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
    if plan.version != Plan::VERSION {
        bail!("unsupported plan version: {}", plan.version);
    }

    preflight(&plan)?;
    if args.dry_run {
        for entry in plan
            .entries
            .iter()
            .filter(|e| e.status == PlanStatus::Ready)
        {
            println!("{} -> {}", entry.source.display(), entry.target.display());
            for move_ in &entry.related {
                println!("  {} -> {}", move_.source.display(), move_.target.display());
            }
        }
        println!(
            "\ndry run: {} ready, {} blocked",
            plan.ready_count(),
            plan.blocked_count()
        );
        return Ok(());
    }

    let journal_path = args
        .journal
        .unwrap_or_else(|| PathBuf::from(".shotrename/journal.jsonl"));
    let batch_id = format!(
        "{}-{}",
        chrono::Local::now().format("%Y%m%dT%H%M%S"),
        std::process::id()
    );
    let mut operations = Vec::new();
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();

    let result = (|| -> Result<()> {
        for entry in plan
            .entries
            .iter()
            .filter(|e| e.status == PlanStatus::Ready)
        {
            if let Some(parent) = entry.target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            let size = fs::metadata(&entry.source)?.len();
            move_file(&entry.source, &entry.target)?;
            done.push((entry.target.clone(), entry.source.clone()));
            operations.push(Operation {
                source: entry.source.clone(),
                target: entry.target.clone(),
                size,
                related: false,
            });
            for move_ in &entry.related {
                let size = fs::metadata(&move_.source)?.len();
                move_file(&move_.source, &move_.target)?;
                done.push((move_.target.clone(), move_.source.clone()));
                operations.push(Operation {
                    source: move_.source.clone(),
                    target: move_.target.clone(),
                    size,
                    related: true,
                });
            }
        }
        Ok(())
    })();

    if let Err(error) = result {
        eprintln!("apply failed: {error:#}");
        let mut rollback_failed = Vec::new();
        for (from, to) in done.iter().rev() {
            if let Err(rollback) = fs::rename(from, to) {
                rollback_failed.push(format!(
                    "{} -> {}: {rollback}",
                    from.display(),
                    to.display()
                ));
            }
        }
        if rollback_failed.is_empty() {
            eprintln!("rolled back {} operation(s)", done.len());
        } else {
            eprintln!("rollback incomplete:");
            for line in rollback_failed {
                eprintln!("  {line}");
            }
        }
        return Err(error);
    }

    let entry = JournalEntry {
        version: JournalEntry::VERSION,
        batch_id,
        created_at: chrono::Local::now().to_rfc3339(),
        operations,
    };
    shotrename_core::journal::append(&journal_path, &entry)?;
    println!(
        "applied {} file(s); journal: {}",
        entry.operations.len(),
        journal_path.display()
    );
    Ok(())
}

fn undo(args: UndoArgs) -> Result<()> {
    let journal_path = args
        .journal
        .unwrap_or_else(|| PathBuf::from(".shotrename/journal.jsonl"));
    let Some(entry) = shotrename_core::journal::last(&journal_path)? else {
        bail!("journal has no batches: {}", journal_path.display());
    };
    for operation in &entry.operations {
        if !operation.target.exists() {
            bail!("missing renamed file: {}", operation.target.display());
        }
        if operation.source.exists() {
            bail!("original path is occupied: {}", operation.source.display());
        }
    }
    if args.dry_run {
        for operation in entry.operations.iter().rev() {
            println!(
                "{} -> {}",
                operation.target.display(),
                operation.source.display()
            );
        }
        return Ok(());
    }
    for operation in entry.operations.iter().rev() {
        move_file(&operation.target, &operation.source)?;
    }
    shotrename_core::journal::remove_last(&journal_path)?;
    println!(
        "undid batch {} ({} file(s))",
        entry.batch_id,
        entry.operations.len()
    );
    Ok(())
}

fn templates() -> Result<()> {
    for (name, body) in [
        ("dji", shotrename_core::BUILTIN_DJI),
        ("insta360", shotrename_core::BUILTIN_INSTA360),
        ("gopro", shotrename_core::BUILTIN_GOPRO),
    ] {
        let template = Template::from_toml(body)?;
        println!("{name:<10} {}", template.filename);
    }
    Ok(())
}

fn build_plan(
    files: &[(PathBuf, PathBuf)],
    template: &Template,
    options: &ProbeOptions,
    output: Option<&Path>,
) -> Result<Plan> {
    let mut allocator = NameAllocator::new();
    let mut entries = Vec::new();
    for (file, root) in files {
        let probe = match probe(file, options) {
            Ok(probe) => probe,
            Err(error) => {
                entries.push(blocked(file, format!("probe failed: {error:#}")));
                continue;
            }
        };
        let fields = probe.field_values(template);
        let rendered = template.render_filename(&fields)?;
        let stem = sanitize_or_renamed(
            &rendered,
            template.format.separator,
            template.format.sanitize,
        );
        let base = output.unwrap_or(root).to_path_buf();
        let target_dir = match template.render_directory(&fields)? {
            Some(directory) => {
                let parts = sanitize_relative_dir(
                    &directory,
                    template.format.separator,
                    template.format.sanitize,
                );
                let mut base = base;
                for part in parts {
                    base.push(part);
                }
                base
            }
            None => base,
        };
        let extension = file
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let current_stem = file
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let date_prefix = {
            let date = fields.lookup("date", None).unwrap_or_default();
            let time = fields.lookup("time", None).unwrap_or_default();
            shotrename_core::sanitize(
                &format!("{date}_{time}"),
                template.format.separator,
                template.format.sanitize,
            )
        };
        let already_organised = target_dir == file.parent().unwrap_or(Path::new("."))
            && (target_dir.join(format!("{stem}.{extension}")) == *file
                || (!date_prefix.is_empty() && current_stem.starts_with(&date_prefix)));
        if already_organised {
            entries.push(PlanEntry {
                source: file.clone(),
                target: file.clone(),
                related: Vec::new(),
                status: PlanStatus::Unchanged,
                reason: None,
                fields: summary_fields(&probe, template),
            });
            continue;
        }
        // Reserve the unchanged name so later entries cannot take it.
        allocator.reserve_existing(file);
        let target = allocator.allocate(&target_dir, &stem, extension);

        let mut related_moves = Vec::new();
        let mut blocked_reason = None;
        if template.related != RelatedMode::Off {
            let target_stem = target
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(stem.as_str());
            for related in related_files(file) {
                let related_ext = related
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                let related_stem = related
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                let main_stem = file
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                let suffix = related_stem
                    .strip_prefix(main_stem)
                    .unwrap_or_default()
                    .trim_start_matches('.');
                let name_stem = if suffix.is_empty() {
                    target_stem.to_string()
                } else {
                    format!("{target_stem}.{suffix}")
                };
                let candidate = format!("{name_stem}.{related_ext}");
                if allocator.is_taken(&target_dir, &candidate) {
                    if template.related == RelatedMode::Strict {
                        blocked_reason = Some(format!(
                            "related file target is occupied: {}",
                            target_dir.join(&candidate).display()
                        ));
                        break;
                    }
                }
                let related_target = allocator.allocate(&target_dir, &name_stem, related_ext);
                related_moves.push(RelatedMove {
                    source: related,
                    target: related_target,
                });
            }
        }
        entries.push(PlanEntry {
            source: file.clone(),
            target,
            related: related_moves,
            status: if blocked_reason.is_some() {
                PlanStatus::Blocked
            } else {
                PlanStatus::Ready
            },
            reason: blocked_reason,
            fields: summary_fields(&probe, template),
        });
    }
    entries.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(Plan {
        version: Plan::VERSION,
        template: template.name.clone(),
        entries,
    })
}

fn summary_fields(probe: &MediaProbe, _template: &Template) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    map.insert("family".into(), probe.family.label().to_string());
    map.insert("camera".into(), probe.camera.clone().unwrap_or_default());
    map.insert(
        "captured".into(),
        probe
            .captured
            .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default(),
    );
    map.insert("time_source".into(), probe.time_source.label().to_string());
    map.insert("resolution".into(), probe.resolution());
    map.insert("fps".into(), probe.fps());
    map.insert("duration".into(), probe.duration());
    map.insert(
        "gps".into(),
        probe
            .gps
            .map(|(lat, lon)| format!("{lat:.6},{lon:.6}"))
            .unwrap_or_default(),
    );
    map
}

fn blocked(path: &Path, reason: String) -> PlanEntry {
    PlanEntry {
        source: path.to_path_buf(),
        target: path.to_path_buf(),
        related: Vec::new(),
        status: PlanStatus::Blocked,
        reason: Some(reason),
        fields: BTreeMap::new(),
    }
}

fn preflight(plan: &Plan) -> Result<()> {
    let mut errors = Vec::new();
    for entry in plan
        .entries
        .iter()
        .filter(|e| e.status == PlanStatus::Ready)
    {
        if !entry.source.is_file() {
            errors.push(format!("missing source: {}", entry.source.display()));
        }
        if entry.target.exists() {
            errors.push(format!("target exists: {}", entry.target.display()));
        }
        for move_ in &entry.related {
            if !move_.source.is_file() {
                errors.push(format!(
                    "missing related source: {}",
                    move_.source.display()
                ));
            }
            if move_.target.exists() {
                errors.push(format!("related target exists: {}", move_.target.display()));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        for error in &errors {
            eprintln!("preflight: {error}");
        }
        bail!("preflight failed with {} problem(s)", errors.len());
    }
}

fn move_file(source: &Path, target: &Path) -> Result<()> {
    fs::rename(source, target)
        .with_context(|| format!("rename {} -> {}", source.display(), target.display()))
}

fn print_plan(plan: &Plan) {
    for entry in &plan.entries {
        match entry.status {
            PlanStatus::Ready => {
                println!("{}", entry.source.display());
                println!("  -> {}", entry.target.display());
                for move_ in &entry.related {
                    println!(
                        "  + {} -> {}",
                        move_.source.display(),
                        move_.target.display()
                    );
                }
            }
            PlanStatus::Blocked => {
                println!("{}", entry.source.display());
                println!("  !! {}", entry.reason.as_deref().unwrap_or("blocked"));
            }
            PlanStatus::Unchanged => {
                println!("{}", entry.source.display());
                println!("  = already matches template");
            }
        }
    }
    println!(
        "\n{} ready, {} blocked, {} unchanged",
        plan.ready_count(),
        plan.blocked_count(),
        plan.unchanged_count()
    );
}

fn collect_files(paths: &[PathBuf]) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_file() {
            if is_supported_video(path) && !is_temporary(path) {
                let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                files.push((path.clone(), root));
            }
            continue;
        }
        if !path.is_dir() {
            bail!("not a file or directory: {}", path.display());
        }
        let root = path.clone();
        for entry in WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                if entry.file_type().is_dir() {
                    let Some(name) = entry.file_name().to_str() else {
                        return false;
                    };
                    !is_excluded_dir(name)
                } else {
                    true
                }
            })
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let candidate = entry.path();
            if is_supported_video(candidate) && !is_temporary(candidate) {
                files.push((candidate.to_path_buf(), root.clone()));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    Ok(files)
}

fn load_template(name: &str) -> Result<Template> {
    let path = Path::new(name);
    if name.ends_with(".toml") || path.is_file() {
        let body = fs::read_to_string(path)
            .with_context(|| format!("read template {}", path.display()))?;
        return Template::from_toml(&body);
    }
    let Some(body) = builtin_template(name) else {
        bail!("unknown template '{name}'. built-ins: dji, insta360, gopro; or pass a .toml path");
    };
    Template::from_toml(body)
}

fn parse_tz(value: &str) -> Result<i32> {
    let value = value.trim();
    let sign = if value.starts_with('-') { -1 } else { 1 };
    let rest = value.trim_start_matches(['+', '-']);
    let (hours, minutes) = match rest.split_once(':') {
        Some((hours, minutes)) => (hours.parse::<i32>()?, minutes.parse::<i32>()?),
        None => (rest.parse::<i32>()?, 0),
    };
    if !(0..=14).contains(&hours) || !(0..60).contains(&minutes) {
        bail!("invalid UTC offset: {value}");
    }
    Ok(sign * (hours * 3600 + minutes * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_offsets() {
        assert_eq!(parse_tz("8").unwrap(), 8 * 3600);
        assert_eq!(parse_tz("+08:00").unwrap(), 8 * 3600);
        assert_eq!(parse_tz("-05:30").unwrap(), -(5 * 3600 + 30 * 60));
    }
}
