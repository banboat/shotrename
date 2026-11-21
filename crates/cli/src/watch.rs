use crate::apply::apply_plan;
use crate::planning::{build_plan, collect_files, load_template, parse_tz, InputFile};
use anyhow::{bail, Context, Result};
use clap::Parser;
use notify::{RecursiveMode, Watcher};
use shotrename_core::{PlanStatus, Template};
use shotrename_meta::{is_excluded_dir, is_supported_video, is_temporary, ProbeOptions};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

#[derive(Debug, Parser)]
pub struct WatchArgs {
    /// Directory to watch (recursively).
    pub path: PathBuf,
    /// Built-in template name (dji, insta360, gopro) or a path to a TOML file.
    #[arg(long, default_value = "dji")]
    pub template: String,
    /// Organise under this directory instead of the watched directory.
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Seconds a file must stay unchanged before it is processed.
    #[arg(long, default_value_t = 3)]
    pub settle: u64,
    /// Process files that already exist before watching.
    #[arg(long)]
    pub no_initial: bool,
    /// Rename files even when they already look organised.
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub journal: Option<PathBuf>,
    #[arg(long)]
    pub tz: Option<String>,
}

struct Pending {
    last_event: Instant,
    size: u64,
}

pub fn run(args: WatchArgs) -> Result<()> {
    let template = load_template(&args.template)?;
    let options = ProbeOptions {
        tz_offset_secs: args.tz.as_deref().map(parse_tz).transpose()?,
    };
    let journal = args
        .journal
        .clone()
        .unwrap_or_else(|| PathBuf::from(".shotrename/journal.jsonl"));
    let settle = Duration::from_secs(args.settle.max(1));

    if !args.no_initial {
        let files = collect_files(std::slice::from_ref(&args.path))?;
        if files.is_empty() {
            println!("no existing files in {}", args.path.display());
        } else {
            process(&files, &template, &options, &args, &journal)?;
        }
    }

    let (sender, receiver) = channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        if let Ok(event) = event {
            let _ = sender.send(event);
        }
    })
    .context("start file watcher")?;
    watcher
        .watch(&args.path, RecursiveMode::Recursive)
        .with_context(|| format!("watch {}", args.path.display()))?;

    println!(
        "watching {} · template {} · settle {}s · ctrl-c to stop",
        args.path.display(),
        template.name,
        settle.as_secs()
    );

    let mut pending: HashMap<PathBuf, Pending> = HashMap::new();
    let mut ignored: HashMap<PathBuf, Instant> = HashMap::new();
    let ignore_for = Duration::from_secs(30);
    loop {
        match receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => {
                for path in event.paths {
                    if !is_supported_video(&path) || is_temporary(&path) {
                        continue;
                    }
                    if inside_excluded(&path, &args.path) {
                        continue;
                    }
                    let Ok(metadata) = fs::metadata(&path) else {
                        continue;
                    };
                    if !metadata.is_file() {
                        continue;
                    }
                    pending.insert(
                        path,
                        Pending {
                            last_event: Instant::now(),
                            size: metadata.len(),
                        },
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("file watcher stopped"),
        }
        ignored.retain(|_, seen| seen.elapsed() < ignore_for);

        let now = Instant::now();
        let mut due = Vec::new();
        for (path, state) in pending.iter_mut() {
            if now.duration_since(state.last_event) < settle {
                continue;
            }
            let Ok(metadata) = fs::metadata(path) else {
                due.push(path.clone());
                continue;
            };
            if metadata.len() != state.size || !metadata.is_file() {
                state.size = metadata.len();
                state.last_event = now;
                continue;
            }
            due.push(path.clone());
        }
        for path in &due {
            pending.remove(path);
        }
        let ready: Vec<PathBuf> = due
            .into_iter()
            .filter(|path| !ignored.contains_key(path))
            .collect();
        if ready.is_empty() {
            continue;
        }

        let inputs: Vec<InputFile> = ready
            .into_iter()
            .map(|path| (path, args.path.clone()))
            .collect();
        let plan = build_plan(
            &inputs,
            &template,
            &options,
            args.output.as_deref(),
            args.force,
        )?;
        for entry in &plan.entries {
            if entry.status == PlanStatus::Ready {
                ignored.insert(entry.target.clone(), Instant::now());
                for move_ in &entry.related {
                    ignored.insert(move_.target.clone(), Instant::now());
                }
            }
        }
        process_plan(&plan, &journal)?;
    }
}

fn process(
    files: &[InputFile],
    template: &Template,
    options: &ProbeOptions,
    args: &WatchArgs,
    journal: &Path,
) -> Result<()> {
    let plan = build_plan(files, template, options, args.output.as_deref(), args.force)?;
    process_plan(&plan, journal)
}

fn process_plan(plan: &shotrename_core::Plan, journal: &Path) -> Result<()> {
    if plan.ready_count() == 0 {
        if plan.blocked_count() > 0 {
            println!("{} file(s) blocked", plan.blocked_count());
        }
        return Ok(());
    }
    let report = apply_plan(plan, journal, false)?;
    println!(
        "{} · {} file(s) renamed",
        chrono::Local::now().format("%H:%M:%S"),
        report.moved
    );
    Ok(())
}

fn inside_excluded(path: &Path, root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    relative
        .parent()
        .map(|parent| {
            parent
                .components()
                .any(|component| is_excluded_dir(&component.as_os_str().to_string_lossy()))
        })
        .unwrap_or(false)
}
