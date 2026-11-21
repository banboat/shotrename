use anyhow::{bail, Context, Result};
use shotrename_core::{journal, JournalEntry, Operation, Plan, PlanStatus};
use std::fs;
use std::path::{Path, PathBuf};

pub struct ApplyReport {
    pub moved: usize,
}

pub fn preflight(plan: &Plan) -> Result<()> {
    let mut errors = Vec::new();
    for entry in plan
        .entries
        .iter()
        .filter(|entry| entry.status == PlanStatus::Ready)
    {
        if !entry.source.is_file() {
            errors.push(format!("missing source: {}", entry.source.display()));
        }
        if entry.target != entry.source && entry.target.exists() {
            errors.push(format!("target exists: {}", entry.target.display()));
        }
        for move_ in &entry.related {
            if !move_.source.is_file() {
                errors.push(format!(
                    "missing related source: {}",
                    move_.source.display()
                ));
            }
            if move_.target != move_.source && move_.target.exists() {
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

pub fn apply_plan(plan: &Plan, journal_path: &Path, dry_run: bool) -> Result<ApplyReport> {
    if plan.version != Plan::VERSION {
        bail!("unsupported plan version: {}", plan.version);
    }
    preflight(plan)?;

    if dry_run {
        for entry in plan
            .entries
            .iter()
            .filter(|entry| entry.status == PlanStatus::Ready)
        {
            if entry.target != entry.source {
                println!("{} -> {}", entry.source.display(), entry.target.display());
            }
            for move_ in &entry.related {
                println!("  {} -> {}", move_.source.display(), move_.target.display());
            }
        }
        println!(
            "\ndry run: {} ready, {} blocked",
            plan.ready_count(),
            plan.blocked_count()
        );
        return Ok(ApplyReport { moved: 0 });
    }

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
            .filter(|entry| entry.status == PlanStatus::Ready)
        {
            move_one(
                &entry.source,
                &entry.target,
                false,
                &mut operations,
                &mut done,
            )?;
            for move_ in &entry.related {
                move_one(
                    &move_.source,
                    &move_.target,
                    true,
                    &mut operations,
                    &mut done,
                )?;
            }
        }
        Ok(())
    })();

    if let Err(error) = result {
        eprintln!("apply failed: {error:#}");
        rollback(&done);
        return Err(error);
    }

    if operations.is_empty() {
        return Ok(ApplyReport { moved: 0 });
    }
    let entry = JournalEntry {
        version: JournalEntry::VERSION,
        batch_id,
        created_at: chrono::Local::now().to_rfc3339(),
        operations,
    };
    if let Err(error) = journal::append(journal_path, &entry) {
        // Without a journal the batch cannot be undone; put the files back.
        eprintln!("journal write failed: {error:#}");
        rollback(&done);
        return Err(error);
    }
    Ok(ApplyReport {
        moved: entry.operations.len(),
    })
}

fn rollback(done: &[(PathBuf, PathBuf)]) {
    let mut failed = Vec::new();
    for (from, to) in done.iter().rev() {
        if let Err(error) = fs::rename(from, to) {
            failed.push(format!("{} -> {}: {error}", from.display(), to.display()));
        }
    }
    if failed.is_empty() {
        eprintln!("rolled back {} operation(s)", done.len());
    } else {
        eprintln!("rollback incomplete:");
        for line in failed {
            eprintln!("  {line}");
        }
    }
}

fn move_one(
    source: &Path,
    target: &Path,
    related: bool,
    operations: &mut Vec<Operation>,
    done: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<()> {
    if source == target {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let size = fs::metadata(source)?.len();
    fs::rename(source, target)
        .with_context(|| format!("rename {} -> {}", source.display(), target.display()))?;
    done.push((target.to_path_buf(), source.to_path_buf()));
    operations.push(Operation {
        source: source.to_path_buf(),
        target: target.to_path_buf(),
        size,
        related,
    });
    Ok(())
}

pub struct UndoReport {
    pub batch_id: String,
    pub restored: usize,
}

pub fn undo_last(journal_path: &Path, dry_run: bool) -> Result<UndoReport> {
    let Some(entry) = journal::last(journal_path)? else {
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
    if dry_run {
        for operation in entry.operations.iter().rev() {
            println!(
                "{} -> {}",
                operation.target.display(),
                operation.source.display()
            );
        }
        return Ok(UndoReport {
            batch_id: entry.batch_id,
            restored: entry.operations.len(),
        });
    }
    for operation in entry.operations.iter().rev() {
        if let Some(parent) = operation.source.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::rename(&operation.target, &operation.source).with_context(|| {
            format!(
                "rename {} -> {}",
                operation.target.display(),
                operation.source.display()
            )
        })?;
    }
    journal::remove_last(journal_path)?;
    Ok(UndoReport {
        batch_id: entry.batch_id,
        restored: entry.operations.len(),
    })
}
