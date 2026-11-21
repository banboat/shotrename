use anyhow::{bail, Context, Result};
use shotrename_core::{
    builtin_template, sanitize, sanitize_or_renamed, sanitize_relative_dir, NameAllocator, Plan,
    PlanEntry, PlanStatus, RelatedMode, RelatedMove, Template,
};
use shotrename_meta::{
    extension,
    group::{self, GroupKind, GroupMember},
    is_excluded_dir, is_supported_video, is_temporary, probe, related_files, MediaProbe,
    ProbeOptions,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// A file plus the input root it was discovered under.
pub type InputFile = (PathBuf, PathBuf);

pub fn collect_files(paths: &[PathBuf]) -> Result<Vec<InputFile>> {
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

struct Item {
    members: Vec<usize>,
    kind: Option<GroupKind>,
}

pub fn build_plan(
    files: &[InputFile],
    template: &Template,
    options: &ProbeOptions,
    output: Option<&Path>,
    force: bool,
) -> Result<Plan> {
    let mut allocator = NameAllocator::new();
    let mut entries = Vec::new();
    let mut probes: Vec<Option<MediaProbe>> = Vec::with_capacity(files.len());
    for (file, _) in files {
        match probe(file, options) {
            Ok(value) => probes.push(Some(value)),
            Err(error) => {
                entries.push(blocked(file, format!("probe failed: {error:#}")));
                probes.push(None);
            }
        }
    }

    let infos: Vec<Option<GroupMember>> = files
        .iter()
        .map(|(file, _)| group::classify(file))
        .collect();
    let mut buckets: BTreeMap<(PathBuf, GroupKind, String), Vec<usize>> = BTreeMap::new();
    for (index, info) in infos.iter().enumerate() {
        if probes[index].is_none() {
            continue;
        }
        if let Some(info) = info {
            let dir = files[index]
                .0
                .parent()
                .unwrap_or(Path::new(""))
                .to_path_buf();
            buckets
                .entry((dir, info.kind, info.key.clone()))
                .or_default()
                .push(index);
        }
    }

    let mut items = Vec::new();
    let mut assigned = vec![false; files.len()];
    for ((_, kind, _), mut members) in buckets {
        members.sort_by(|left, right| {
            let rank = |index: usize| infos[index].as_ref().map(|info| info.rank).unwrap_or(0);
            rank(*left)
                .cmp(&rank(*right))
                .then_with(|| files[*left].0.cmp(&files[*right].0))
        });
        for index in &members {
            assigned[*index] = true;
        }
        items.push(Item {
            members,
            kind: Some(kind),
        });
    }
    for index in 0..files.len() {
        if probes[index].is_some() && !assigned[index] {
            items.push(Item {
                members: vec![index],
                kind: None,
            });
        }
    }
    items.sort_by(|left, right| files[left.members[0]].0.cmp(&files[right.members[0]].0));

    for item in &items {
        build_item(
            &mut allocator,
            files,
            &probes,
            &infos,
            item,
            template,
            output,
            force,
            &mut entries,
        )?;
    }

    entries.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(Plan {
        version: Plan::VERSION,
        template: template.name.clone(),
        entries,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_item(
    allocator: &mut NameAllocator,
    files: &[InputFile],
    probes: &[Option<MediaProbe>],
    infos: &[Option<GroupMember>],
    item: &Item,
    template: &Template,
    output: Option<&Path>,
    force: bool,
    entries: &mut Vec<PlanEntry>,
) -> Result<()> {
    let representative = item.members[0];
    let probe = probes[representative]
        .as_ref()
        .expect("items only contain probed files");
    let fields = probe.field_values(template);
    let rendered = template.render_filename(&fields)?;
    let stem = sanitize_or_renamed(
        &rendered,
        template.format.separator,
        template.format.sanitize,
    );
    let base_root = output.unwrap_or(&files[representative].1).to_path_buf();
    let target_dir = match template.render_directory(&fields)? {
        Some(directory) => {
            let parts = sanitize_relative_dir(
                &directory,
                template.format.separator,
                template.format.sanitize,
            );
            let mut base = base_root;
            for part in parts {
                base.push(part);
            }
            base
        }
        None => base_root,
    };
    let date_prefix = {
        let date = fields.lookup("date", None).unwrap_or_default();
        let time = fields.lookup("time", None).unwrap_or_default();
        sanitize(
            &format!("{date}_{time}"),
            template.format.separator,
            template.format.sanitize,
        )
    };

    let group_label = item.kind.map(|kind| match kind {
        GroupKind::Insta360Lens => {
            let lenses: Vec<&str> = item
                .members
                .iter()
                .filter_map(|index| infos[*index].as_ref())
                .map(|info| info.lens.as_str())
                .collect();
            format!("insta360 {}", lenses.join("/"))
        }
        GroupKind::GoProChapter => format!("gopro {} chapter(s)", item.members.len()),
    });

    struct Spec {
        probe_index: usize,
        source: PathBuf,
        extension: String,
        suffix: String,
    }
    let member_infos: Vec<GroupMember> = if item.kind.is_some() {
        item.members
            .iter()
            .map(|index| {
                infos[*index]
                    .clone()
                    .expect("grouped items only contain classified files")
            })
            .collect()
    } else {
        Vec::new()
    };
    let chapters = if item.kind == Some(GroupKind::GoProChapter) {
        group::chapter_labels(&member_infos)
    } else {
        vec![0; member_infos.len()]
    };
    let specs: Vec<Spec> = item
        .members
        .iter()
        .enumerate()
        .map(|(position, index)| {
            let suffix = if item.kind.is_some() {
                member_infos[position]
                    .suffix(chapters[position], item.members.len())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            Spec {
                probe_index: *index,
                source: files[*index].0.clone(),
                extension: extension(&files[*index].0),
                suffix,
            }
        })
        .collect();
    let owned: Vec<PathBuf> = specs.iter().map(|spec| spec.source.clone()).collect();
    let members: Vec<(String, String)> = specs
        .iter()
        .map(|spec| (spec.suffix.clone(), spec.extension.clone()))
        .collect();

    let provisional: Vec<PathBuf> = specs
        .iter()
        .map(|spec| target_dir.join(compose_name(&stem, &spec.suffix, &spec.extension)))
        .collect();
    let organised = |source: &Path, target: &Path| -> bool {
        if target == source {
            return true;
        }
        !force
            && !date_prefix.is_empty()
            && target_dir == source.parent().unwrap_or(Path::new("."))
            && source
                .file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(|current| current.starts_with(&date_prefix))
    };
    let all_unchanged = specs
        .iter()
        .zip(&provisional)
        .all(|(spec, target)| organised(&spec.source, target));

    let base = if all_unchanged {
        // Nothing moves; skip allocation so names stay free for other items.
        stem.clone()
    } else {
        allocator.allocate_group(&target_dir, &stem, &members, &owned)
    };

    for spec in &specs {
        let target = target_dir.join(compose_name(&base, &spec.suffix, &spec.extension));
        let unchanged = organised(&spec.source, &target);
        let mut related_moves = Vec::new();
        let mut blocked_reason = None;
        if !unchanged && template.related != RelatedMode::Off {
            let target_stem = target
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let main_stem = spec
                .source
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            for related in related_files(&spec.source) {
                let related_ext = extension(&related);
                let related_stem = related
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
                let candidate = if related_ext.is_empty() {
                    name_stem.clone()
                } else {
                    format!("{name_stem}.{related_ext}")
                };
                if target_dir.join(&candidate) == related {
                    continue;
                }
                if allocator.is_taken(&target_dir, &candidate)
                    && template.related == RelatedMode::Strict
                {
                    blocked_reason = Some(format!(
                        "related file target is occupied: {}",
                        target_dir.join(&candidate).display()
                    ));
                    break;
                }
                let related_target = allocator.allocate(&target_dir, &name_stem, &related_ext);
                related_moves.push(RelatedMove {
                    source: related,
                    target: related_target,
                });
            }
        }
        let member_probe = probes[spec.probe_index]
            .as_ref()
            .expect("items only contain probed files");
        let mut fields_map = summary_fields(
            member_probe,
            &spec.source,
            &spec.suffix,
            group_label.as_deref(),
        );
        fields_map.insert("unchanged".into(), unchanged.to_string());
        let status = if blocked_reason.is_some() {
            PlanStatus::Blocked
        } else if unchanged {
            PlanStatus::Unchanged
        } else {
            PlanStatus::Ready
        };
        let target = if unchanged {
            spec.source.clone()
        } else {
            target
        };
        entries.push(PlanEntry {
            source: spec.source.clone(),
            target,
            related: related_moves,
            status,
            reason: blocked_reason,
            fields: fields_map,
        });
    }
    Ok(())
}

fn compose_name(base: &str, suffix: &str, extension: &str) -> String {
    if extension.is_empty() {
        format!("{base}{suffix}")
    } else {
        format!("{base}{suffix}.{extension}")
    }
}

fn summary_fields(
    probe: &MediaProbe,
    source: &Path,
    suffix: &str,
    group_label: Option<&str>,
) -> BTreeMap<String, String> {
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
    map.insert(
        "location".into(),
        probe
            .place
            .as_ref()
            .map(|place| place.name.clone())
            .unwrap_or_default(),
    );
    map.insert("size".into(), human_size(probe.size));
    map.insert(
        "file".into(),
        source
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default(),
    );
    if !suffix.is_empty() {
        map.insert("suffix".into(), suffix.to_string());
    }
    if let Some(label) = group_label {
        map.insert("group".into(), label.to_string());
    }
    map
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
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

pub fn load_template(name: &str) -> Result<Template> {
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

pub fn parse_tz(value: &str) -> Result<i32> {
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

pub fn print_plan(plan: &Plan) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::apply_plan;
    use shotrename_core::PlanStatus;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "shotrename-cli-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn plan_for(dir: &Path, template: &str) -> Plan {
        let files = collect_files(&[dir.to_path_buf()]).unwrap();
        build_plan(
            &files,
            &load_template(template).unwrap(),
            &ProbeOptions::default(),
            None,
            false,
        )
        .unwrap()
    }

    fn names(plan: &Plan) -> Vec<String> {
        plan.entries
            .iter()
            .filter(|entry| entry.status == PlanStatus::Ready)
            .map(|entry| {
                entry
                    .target
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn groups_insta360_pair_and_proxies() {
        let dir = temp_dir("insta");
        for name in [
            "VID_20241121_143015_00_001.insv",
            "VID_20241121_143015_10_001.insv",
            "LRV_20241121_143015_00_001.lrv",
            "LRV_20241121_143015_10_001.lrv",
        ] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let plan = plan_for(&dir, "insta360");
        let targets = names(&plan);
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|name| name.ends_with("_00.insv")));
        assert!(targets.iter().any(|name| name.ends_with("_10.insv")));
        let bases: Vec<&str> = targets
            .iter()
            .map(|name| {
                name.trim_end_matches(|c: char| c != '_')
                    .trim_end_matches('_')
            })
            .collect();
        assert_eq!(bases[0], bases[1]);
        let related: Vec<String> = plan
            .entries
            .iter()
            .flat_map(|entry| &entry.related)
            .map(|move_| {
                move_
                    .target
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(related.len(), 2);
        assert!(related.iter().all(|name| name.ends_with(".lrv")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn groups_gopro_chapters() {
        let dir = temp_dir("gopro");
        for name in ["GX010123.MP4", "GX020123.MP4"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let plan = plan_for(&dir, "gopro");
        let targets = names(&plan);
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|name| name.ends_with("_ch01.mp4")));
        assert!(targets.iter().any(|name| name.ends_with("_ch02.mp4")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn replan_is_idempotent_for_grouped_files() {
        let dir = temp_dir("idempotent");
        for name in [
            "VID_20241121_143015_00_001.insv",
            "VID_20241121_143015_10_001.insv",
            "LRV_20241121_143015_00_001.lrv",
            "LRV_20241121_143015_10_001.lrv",
        ] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let plan = plan_for(&dir, "insta360");
        let journal = dir.join("journal.jsonl");
        apply_plan(&plan, &journal, false).unwrap();
        let again = plan_for(&dir, "insta360");
        assert_eq!(again.ready_count(), 0);
        assert_eq!(again.unchanged_count(), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fills_location_from_srt_gps() {
        let dir = temp_dir("location");
        fs::write(dir.join("DJI_0001.MP4"), b"x").unwrap();
        fs::write(
            dir.join("DJI_0001.SRT"),
            "1\n00:00:00,033 --> 00:00:00,066\n2024-11-21 14:30:15.000\n\
             [latitude: 30.2741] [longitude: 120.1551] [altitude: 20.1]\n",
        )
        .unwrap();
        let plan = plan_for(&dir, "dji");
        let entry = &plan.entries[0];
        assert_eq!(
            entry.fields.get("location").map(String::as_str),
            Some("Hangzhou")
        );
        assert!(entry
            .target
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("Hangzhou"));
        let _ = fs::remove_dir_all(dir);
    }
}
