# dota2-highlight-recorder

**English** · [简体中文](README.zh-CN.md)

A Windows desktop tool that reads a Dota 2 match replay, finds a player's highlights automatically,
then drives the game to play them back from that player's view and records them to video through OBS.

Reference: https://github.com/DrEAmSs59/CS2-insight-agent (a similar tool for CS2; used for ideas only, no code copied)

## Features

1. **Import a replay**: enter a match ID or pick a local replay file
2. **Pick a player**: list the 10 players and heroes in the match and choose who to record; defaults to you, but anyone can be picked
3. **Find highlights**: find the player's kills, kill streaks and multi-kills, and merge kills that are close together into one clip
4. **Preview the plan**: list every clip before recording and untick the ones you don't want
5. **Record automatically**: control the replay and OBS to record each clip from the player's view; idle time between kills inside a clip is cut out
6. **Output videos**: one video per clip by default, with file names that explain themselves; optionally joined into a single compilation

## Principles

- No injection into the game process, no changes to game files
- Any game settings changed for recording are restored afterwards
- Validate first: prove that replay control and recording work before writing the real code

## Tech stack

| Part | Choice |
|---|---|
| Desktop app | Tauri 2 + Rust, plain HTML/CSS/JS front end |
| Replay parsing | [source2-demo](https://github.com/Rupas1k/source2-demo) (fallback: [Clarity](https://github.com/skadistats/clarity)) |
| Playback control | Game console over netcon (`-netconport`, requires `-insecure`); spectator UI via window messages (keys/clicks) |
| Playback state | Game State Integration (GSI) |
| Recording | OBS (obs-websocket v5) |
| Cutting / joining | ffmpeg |

## Roadmap

### Phase 0: Technical validation

Each item is a small standalone experiment; formal development starts only after they all work.

- [x] **Get the replay**: obtain the `.dem` file from a match ID (replay URL, expiry, local replay folder) — [notes (zh-CN)](docs/spikes/01-replay.md)
- [x] **Parse the replay**: read the 10 players and heroes, plus kill events and their game time, from the `.dem` file — [notes (zh-CN)](docs/spikes/01-replay.md)
- [x] **Control playback**: without injecting into the process, make the game load a replay, jump to a given time and lock the camera to a given player; also check whether a command can make the client download a replay (it can't) — [notes (zh-CN)](docs/spikes/02-playback.md)
- [ ] **Read playback state**: confirm the actual game time after a jump, to line up recording start and end
- [ ] **Control OBS**: start, stop and pause recording via obs-websocket, and get the output file
- [ ] **End to end**: given a time range by hand, automatically record that stretch from the player's view

### Phase 1: Command-line MVP

- [ ] Highlight detection and clip-merging rules
- [ ] Recording plan generation and execution
- [ ] Back up and restore game settings
- [ ] Output file naming, optional compilation

### Phase 2: Desktop UI

- [ ] Import, player selection, plan preview, recording progress, output management

## License

[PolyForm Noncommercial 1.0.0](LICENSE): personal, study, research and nonprofit use is allowed; commercial use is not.
