# dota2-highlight-recorder

**English** · [简体中文](README.zh-CN.md)

A Windows tool that reads a Dota 2 match replay, finds a player's highlights automatically,
then drives the game to play them back from that player's view and records them to video with the game's built-in clip builder.

Reference: https://github.com/DrEAmSs59/CS2-insight-agent (a similar tool for CS2; used for ideas only, no code copied)

## Features

1. **Import a replay**: enter a match ID or pick a local replay file
2. **Pick a player**: list the 10 players and heroes in the match and choose who to record; defaults to you, but anyone can be picked
3. **Find highlights**: find the player's kills, kill streaks and multi-kills, and merge kills that are close together into one clip
4. **Preview the plan**: list every clip before recording and untick the ones you don't want
5. **Record automatically**: control the replay and the clip builder to record each clip from the player's view; idle time between kills inside a clip is cut out
6. **Output videos**: one video per clip by default, with file names that explain themselves; optionally joined into a single compilation

## Principles

- No injection into the game process; no changes to game files, except a GSI config file written for the recording session and deleted afterwards
- Any game settings changed for recording are restored afterwards
- Validate first: prove that replay control and recording work before writing the real code

## Usage

This is the command-line version (phase 1); the desktop UI comes in phase 2.

### Before you start

- Windows, Dota 2 installed, Steam logged in
- Download the replay in the game first: find the match under Watch → Match History and click Download
- [ffmpeg](https://ffmpeg.org/download.html) (for Windows, the builds by BtbN or gyan.dev work): pass `ffmpeg.exe` or its folder with `--ffmpeg`, or set the `D2HR_FFMPEG` environment variable, or put it on PATH
- Building from source needs [Rust](https://rustup.rs/):

```bash
cargo build --release -p d2hr
```

The program is `target\release\d2hr.exe`. Its messages are in Chinese for now.

### Commands

```bash
d2hr list
```

Lists the replays the game has downloaded: match ID, when it was downloaded, length, and the hero you played with the result.

```bash
d2hr plan 9052676196
```

Prints your highlight clips in that match (you = the account logged in to Steam): time range, heroes killed, number of exports, output file name, and the estimated recording time. Use `--player` to pick someone else (slot 0–9 or account ID).

```bash
d2hr record 9052676196 --compile --ffmpeg C:\tools\ffmpeg\bin
```

Records the clips, one video per clip; `--compile` also joins them into a compilation. Common options:

| Option | What it does |
|---|---|
| `--player <slot or account>` | Who to record; defaults to you |
| `--clips 1,3` | Only record these clips (numbers from `plan`); defaults to all |
| `--compile` | Also join all clips into one compilation |
| `--resolution 1920x1080` | Recording resolution; defaults to the game window size |
| `--out <folder>` | Output folder; defaults to `Videos\d2hr` |
| `--quit` | Quit the game when done |
| `--anchor` `--max-lead` `--post` `--merge-gap` `--keep-gap` … | Tune the clip rules (also work with `plan`; see `d2hr plan --help`) |

File names look like `9052676196_7_phantom_assassin_31-57_32-13_三杀.mp4`: match ID, player slot, hero, start and end time, and the multi-kill tag (in Chinese, e.g. 三杀 = triple kill).

### What happens while recording

- **Don't touch the mouse or keyboard once recording starts**, until the command finishes. The program switches to the game to click its UI (spectator menu, clip builder), then waits for rendering; each clip takes roughly half a minute to a minute, and `plan` tells you the estimate up front.
- Dota is started with `-netconport -insecure` (plus the launch options you set in Steam). A game started with `-insecure` can't join matchmaking, so quit it after recording and start Dota from Steam as usual. If Dota is already running normally, the program asks you to quit it first.
- A few game settings are changed for the session (the clip builder's output folder and timeline length, spectator edge-pan camera) and a GSI config file is written; all of it is restored or removed at the end, including after errors and Ctrl+C. With `--resolution`, the game is quit after recording and the window size in your display settings is put back.
- To stop early, press Ctrl+C once: the program finishes the current step, stops and restores the settings as usual. Pressing it again exits immediately, without restoring.

### Known limitations

- Click positions have been tested in 2560×1600, 1920×1080 and 1600×900 windows and in 2560×1600 exclusive fullscreen; other resolutions may miss.
- The spectator hotkeys for "focus player N" must be single keys (the default is the number keys 1–0); combinations with Shift or Alt are not supported.
- Continuous action longer than 53 s is exported in parts and joined; the joins may repeat or skip up to about 1 s.

## Tech stack

| Part | Choice |
|---|---|
| Desktop app | Tauri 2 + Rust, plain HTML/CSS/JS front end |
| Replay parsing | [source2-demo](https://github.com/Rupas1k/source2-demo) (fallback: [haste](https://github.com/blukai/haste) (Rust) or [manta](https://github.com/dotabuff/manta) (Go)) |
| Playback control | Game console over netcon (`-netconport`, requires `-insecure`); spectator UI via window messages (keys/clicks) |
| Playback state | Game State Integration (GSI) |
| Recording | Dota 2's built-in replay clip builder (≤ 60 s per export; longer clips exported in parts and joined) — [notes (zh-CN)](docs/spikes/06-clip-builder.md) |
| Recording (fallback) | OBS (obs-websocket v5), game capture (only in `-insecure` sessions; capture source removed after recording) |
| Cutting / joining | ffmpeg |

## Roadmap

### Phase 0: Technical validation

Each item is a small standalone experiment; formal development starts only after they all work.

- [x] **Get the replay**: obtain the `.dem` file from a match ID (replay URL, expiry, local replay folder) — [notes (zh-CN)](docs/spikes/01-replay.md)
- [x] **Parse the replay**: read the 10 players and heroes, plus kill events and their game time, from the `.dem` file — [notes (zh-CN)](docs/spikes/01-replay.md)
- [x] **Control playback**: without injecting into the process, make the game load a replay, jump to a given time and lock the camera to a given player; also check whether a command can make the client download a replay (it can't) — [notes (zh-CN)](docs/spikes/02-playback.md)
- [x] **Read playback state**: confirm the actual game time after a jump, to line up recording start and end — [notes (zh-CN)](docs/spikes/03-state.md)
- [x] **Control OBS**: start, stop and pause recording via obs-websocket, and get the output file — [notes (zh-CN)](docs/spikes/04-obs.md)
- [x] **End to end**: given a time range by hand, automatically record that stretch from the player's view — [notes (zh-CN)](docs/spikes/05-e2e.md)

### Phase 1: Command-line MVP

Plan: [docs/phase1-plan.md](docs/phase1-plan.md) (zh-CN).

- [x] Highlight detection and clip-merging rules (each clip starts when the fight actually started; defaults chosen from statistics over 17 local replays)
- [x] Recording plan generation and execution (`d2hr plan` / `d2hr record`)
- [x] Back up and restore game settings (including after errors and Ctrl+C)
- [x] Output file naming, optional compilation

### Phase 2: Desktop UI

- [ ] Import, player selection, plan preview, recording progress, output management

## License

[PolyForm Noncommercial 1.0.0](LICENSE): personal, study, research and nonprofit use is allowed; commercial use is not.
