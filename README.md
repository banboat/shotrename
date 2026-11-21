# ShotRename

**Batch rename videos by what's inside them.** Local-first, open source.

> Your camera names files `DJI_0001.MP4`. ShotRename names them by date, camera, GPS and resolution.

ShotRename reads what the file already knows: capture time, camera, lens, GPS, duration, resolution, frame rate, plus sidecar files. It then proposes a clean, consistent name. Content-aware titles are planned as an optional local model; nothing is uploaded.

ShotRename is an open-source alternative to closed-source video renamers such as RenamerX.

## Status

v0.1 core CLI works end to end:

- scan `DJI_*`, `VID_*.insv`, `GX*` and generic footage;
- read MP4/MOV atoms, DJI SRT telemetry, FFprobe and ExifTool when installed;
- render TOML templates and preview a plan without touching files;
- rename videos and sidecars, with conflict suffixes and an undo journal;
- undo the last batch.

Not done yet: desktop GUI, watch folders, Insta360 dual-lens pairing, offline reverse geocoding, AI titles.

## Install

```bash
git clone https://github.com/banboat/shotrename
cd shotrename
cargo build --release
./target/release/shotrename --help
```

FFprobe and ExifTool are optional. Basic naming works with native MP4 atom parsing and DJI SRT files.

## Usage

```bash
# 1. Inspect what ShotRename can read
shotrename scan ./footage --tz +08:00

# 2. Build a plan (never writes to disk)
shotrename plan ./footage --template dji --tz +08:00 --out plan.json

# 3. Apply, with a journal for undo
shotrename apply plan.json --journal .shotrename/journal.jsonl

# 4. Undo the last batch
shotrename undo --journal .shotrename/journal.jsonl
```

Example plan:

```text
footage/DCIM/100MEDIA/DJI_0001.MP4
  -> footage/2024/11/2024-11-21_143015_DJI_Osmo_Action_5_3840x2160.MP4
  + footage/DCIM/100MEDIA/DJI_0001.SRT -> footage/2024/11/2024-11-21_143015_DJI_Osmo_Action_5_3840x2160.SRT
```

`plan` is idempotent: files already named by the template are marked `unchanged` instead of being renamed twice.

## Templates

Templates are TOML, so they can live in Git and be reviewed like code:

```toml
name = "dji-default"
filename = "{date}_{time}_{camera}_{location}_{resolution}"
directory = "{date:%Y}/{date:%m}"
related = "strict"

[format]
date = "%Y-%m-%d"
time = "%H%M%S"
separator = "_"
sanitize = "unicode"
```

Fields: `date`, `time`, `camera`, `lens`, `location`, `resolution`, `fps`, `duration`, `codec`, `original`, `family`, `width`, `height`, `altitude`, `seq`.

`{date:%Y}` uses chrono's strftime syntax. Empty fields collapse together with their separators.

Built-in templates: `dji`, `insta360`, `gopro`. List them with `shotrename templates`.

## Design principles

1. Local-first. Footage does not leave the machine.
2. Deterministic by default. Metadata first; AI is optional.
3. Preview before apply. No silent overwrites.
4. Undo everything. Every apply writes a journal.
5. Templates as data. TOML files that can live in Git.
6. Sidecars travel with their video. `.LRF`, `.LRV`, `.SRT`, `.WAV`, `.THM`, including language-tagged subtitles.

## Roadmap

- [x] CLI: scan, plan, apply, undo
- [x] MP4 atoms, DJI SRT, FFprobe/ExifTool adapters
- [x] Sidecar files and conflict suffixes
- [ ] Insta360 dual-lens pairing (`.insv` `_00_` + `_10_`)
- [ ] GoPro chapter groups (`GX01`/`GX02`)
- [ ] Offline location names from GPS
- [ ] Watch folders with configurable settle time
- [ ] Tauri desktop app
- [ ] Optional local AI titles

## Development

```bash
cargo test          # unit tests for naming, sanitising, conflicts, MP4 and SRT parsing
cargo build
```

Workspace layout:

```text
crates/core   templates, rendering, sanitising, conflict allocation, journal
crates/meta   filename detection, MP4 atoms, DJI SRT, FFprobe, ExifTool
crates/cli    clap commands
templates/    built-in TOML templates
```

## License

MIT. See [LICENSE](LICENSE).

## Not affiliated

ShotRename is an independent open-source project. It is not affiliated with RenamerX, Rename X, or any other product.

---

## 中文

**按视频内容批量重命名，本地优先，开源。**

v0.1 的 CLI 已能跑通：扫描大疆、影石、GoPro 和通用视频，读 MP4 元数据、大疆 SRT 遥测、FFprobe/ExifTool，按 TOML 模板生成计划，预览后应用，支持关联文件和撤销日志。

已支持 `.LRF`、`.LRV`、`.SRT`、`.WAV`、`.THM` 跟随主视频，包含 `DJI_0001.en.srt` 这类语言后缀。重复规划会把已符合模板的文件标为 `unchanged`，不会二次改名。

尚未完成：桌面端、监控文件夹、影石双镜头 `.insv` 成组、GPS 离线地名、可选的本地 AI 标题。
