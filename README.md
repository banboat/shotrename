# ShotRename

**Batch rename videos by what's inside them.** Local-first, open source.

> Your camera names files `DJI_0001.MP4`. ShotRename names them by date, camera, location, and content.

ShotRename is an early-stage project. This repository is the initial placeholder for the CLI and desktop app. It is part of the [banboat tools](https://github.com/banboat/tools) collection.

## Why

Action cameras, drones, and phones produce opaque filenames. Renaming a shoot by hand is slow and error-prone. Existing tools are often GUI-only, paid, or upload footage to a cloud service.

ShotRename reads what the file already knows: capture time, camera, lens, GPS, duration, resolution, frame rate, and sidecar files. It then proposes a clean, consistent name. Content-aware naming (scene, speech, on-screen text) is optional and runs locally.

ShotRename is an open-source alternative to closed-source video renamers such as RenamerX.

## Planned features

- Scan DJI, Insta360, GoPro, and generic camera footage.
- Group sidecar files (`.LRF`, `.WAV`, `.SRT`, `.LRV`) with their video.
- Read metadata with FFprobe and ExifTool; parse DJI SRT telemetry.
- Template engine: fields, order, separator, date format, folder organization.
- Dry-run preview; never overwrite; append a sequence number on conflict.
- Apply and undo with a local journal.
- CLI first; Tauri desktop later.
- Optional local AI for content titles; no cloud by default.

## CLI (planned)

```bash
shotrename scan ./footage
shotrename plan --template dji-default
shotrename apply --journal .shotrename/journal.json
shotrename undo --last
```

## Design principles

1. Local-first. Footage does not leave the machine.
2. Deterministic by default. Metadata first; AI is optional.
3. Preview before apply. No silent overwrites.
4. Undo everything. Every apply writes a journal.
5. Templates as data. TOML/JSON files that can live in Git.

## Roadmap

- [ ] CLI MVP: scan, plan, apply, undo
- [ ] DJI and Insta360 metadata parsers
- [ ] Template library and conflict handling
- [ ] Tauri desktop app
- [ ] Optional local content model

## Status

Early development. No installable release yet. Watch the repository or open an issue with a sample filename pattern.

## License

TBD. MIT or Apache-2.0 is the intended direction.

## Not affiliated

ShotRename is an independent open-source project. It is not affiliated with RenamerX, Rename X, or any other product.

---

## 中文

**按视频内容批量重命名，本地优先，开源。**

大疆、影石、GoPro 和手机拍出来的文件名通常没有意义。ShotRename 读取拍摄时间、机型、镜头、GPS、时长、分辨率、帧率和同名侧车文件，生成一致、可搜索的文件名。内容理解（场景、语音、画面文字）是可选能力，默认在本机运行。

计划支持：DJI / Insta360 / GoPro / 通用视频；`.LRF`、`.WAV`、`.SRT`、`.LRV` 跟随主视频；预览、冲突追加序号、应用与撤销；CLI 优先，桌面端后续。
