//! 录制记录：每次录制（录完、停下或出错）记一笔，存在 `%APPDATA%\dota2-highlight-recorder\history.json`。
//! 只存元数据（比赛、玩家、片段、文件路径、结果），不存视频。界面上按比赛汇总显示（"已录视频"）。
//!
//! 同一个片段按文件名认：文件名由比赛、玩家、起止时间决定，重录会覆盖同名文件，所以只看最新录成的那次。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 读改写 history.json 时加锁，免得录完写入和界面删除撞在一起
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone)]
pub struct Run {
    /// 开始时间（Unix 秒）
    pub time: u64,
    pub match_id: u64,
    pub player: usize,
    /// 英雄内部名（不带前缀）
    pub hero: String,
    pub player_name: String,
    pub is_me: bool,
    /// 这次选了要录的片段
    pub clips: Vec<RunClip>,
    /// 合集文件；没拼合集为 None
    pub compilation: Option<PathBuf>,
    /// 停下的原因（文字）；录完或出错为 None
    pub cancelled: Option<String>,
    pub error: Option<String>,
    pub elapsed_secs: f32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RunClip {
    /// 这次计划里的序号
    pub index: usize,
    /// 文件名（不含扩展名），同一片段的标识
    pub file_name: String,
    pub start: f32,
    pub end: f32,
    pub victims: Vec<String>,
    pub multi_kill: u32,
    /// 录成的文件；没录完为 None
    pub path: Option<PathBuf>,
}

fn path() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(|d| PathBuf::from(d).join(r"dota2-highlight-recorder\history.json"))
}

fn load() -> Vec<Run> {
    path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn store(runs: &[Run]) -> Result<(), String> {
    let p = path().ok_or("找不到 APPDATA 目录")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(runs).map_err(|e| e.to_string())?;
    // 先写临时文件再改名，写到一半退出也不会把整个记录弄坏
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写不了 {}：{e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("写不了 {}：{e}", p.display()))
}

/// 记一笔（录制线程结束时调用）
pub fn add(run: Run) -> Result<(), String> {
    let _g = LOCK.lock().unwrap();
    let mut runs = load();
    runs.push(run);
    store(&runs)
}

/// 一个文件的状态：在不在
#[derive(Serialize)]
pub struct FileRecord {
    path: PathBuf,
    exists: bool,
}

impl FileRecord {
    fn new(path: &Path) -> Self {
        FileRecord { path: path.to_owned(), exists: path.is_file() }
    }
}

#[derive(Serialize)]
pub struct ClipRecord {
    file_name: String,
    /// 最近一次录制计划里的序号（规则改过的话，比赛页上的序号可能不同）
    index: usize,
    start: f32,
    end: f32,
    victims: Vec<String>,
    multi_kill: u32,
    /// 最近一次录成的文件；从没录成为 None（"未录完"）
    file: Option<FileRecord>,
}

#[derive(Serialize)]
pub struct PlayerRecord {
    player: usize,
    hero: String,
    player_name: String,
    is_me: bool,
    /// 按开始时间排序
    clips: Vec<ClipRecord>,
    /// 最近一次拼的合集
    compilation: Option<FileRecord>,
}

#[derive(Serialize)]
pub struct RunSummary {
    time: u64,
    player: usize,
    hero: String,
    /// 这次选的片段序号、录成的序号
    selected: Vec<usize>,
    completed: Vec<usize>,
    cancelled: Option<String>,
    error: Option<String>,
    elapsed_secs: f32,
}

#[derive(Serialize)]
pub struct MatchRecord {
    match_id: u64,
    /// 最近一次录制的时间
    last_time: u64,
    players: Vec<PlayerRecord>,
    /// 最近的在前
    runs: Vec<RunSummary>,
}

/// 按比赛汇总的录制记录，最近录过的比赛在前
#[tauri::command]
pub fn recorded_matches() -> Vec<MatchRecord> {
    let runs = {
        let _g = LOCK.lock().unwrap();
        load()
    };
    let mut by_match: Vec<(u64, Vec<&Run>)> = Vec::new();
    for r in &runs {
        match by_match.iter_mut().find(|(id, _)| *id == r.match_id) {
            Some((_, list)) => list.push(r),
            None => by_match.push((r.match_id, vec![r])),
        }
    }
    let mut out: Vec<MatchRecord> = by_match.into_iter().map(|(match_id, list)| summarize(match_id, &list)).collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.last_time));
    out
}

/// `runs` 是同一场比赛的记录，按时间从早到晚
fn summarize(match_id: u64, runs: &[&Run]) -> MatchRecord {
    let mut players: Vec<PlayerRecord> = Vec::new();
    for run in runs {
        let p = match players.iter().position(|p| p.player == run.player) {
            Some(i) => &mut players[i],
            None => {
                players.push(PlayerRecord {
                    player: run.player,
                    hero: run.hero.clone(),
                    player_name: run.player_name.clone(),
                    is_me: run.is_me,
                    clips: Vec::new(),
                    compilation: None,
                });
                players.last_mut().unwrap()
            }
        };
        // 后录的覆盖先录的：序号用最新的，文件只在这次录成时更新
        for c in &run.clips {
            let file = c.path.as_deref().map(FileRecord::new);
            match p.clips.iter_mut().find(|r| r.file_name == c.file_name) {
                Some(r) => {
                    r.index = c.index;
                    if file.is_some() {
                        r.file = file;
                    }
                }
                None => p.clips.push(ClipRecord {
                    file_name: c.file_name.clone(),
                    index: c.index,
                    start: c.start,
                    end: c.end,
                    victims: c.victims.clone(),
                    multi_kill: c.multi_kill,
                    file,
                }),
            }
        }
        if let Some(c) = &run.compilation {
            p.compilation = Some(FileRecord::new(c));
        }
    }
    for p in &mut players {
        p.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
    }
    // 我在前，其余按玩家序号
    players.sort_by_key(|p| (!p.is_me, p.player));
    MatchRecord {
        match_id,
        last_time: runs.iter().map(|r| r.time).max().unwrap_or(0),
        players,
        runs: runs
            .iter()
            .rev()
            .map(|r| RunSummary {
                time: r.time,
                player: r.player,
                hero: r.hero.clone(),
                selected: r.clips.iter().map(|c| c.index).collect(),
                completed: r.clips.iter().filter(|c| c.path.is_some()).map(|c| c.index).collect(),
                cancelled: r.cancelled.clone(),
                error: r.error.clone(),
                elapsed_secs: r.elapsed_secs,
            })
            .collect(),
    }
}

/// 删掉一场比赛的全部记录。`delete_files` 时把记录里的视频移到回收站，返回移走了几个
#[tauri::command]
pub fn delete_match_record(match_id: u64, delete_files: bool) -> Result<usize, String> {
    let _g = LOCK.lock().unwrap();
    let mut runs = load();
    let (gone, keep): (Vec<Run>, Vec<Run>) = runs.drain(..).partition(|r| r.match_id == match_id);
    let mut moved = 0;
    if delete_files {
        let mut files: Vec<PathBuf> = gone
            .iter()
            .flat_map(|r| r.clips.iter().filter_map(|c| c.path.clone()).chain(r.compilation.clone()))
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        files.dedup();
        recycle(&files)?;
        moved = files.len();
    }
    store(&keep)?;
    Ok(moved)
}

/// 把文件移到回收站（不是永久删除）
pub(crate) fn recycle(files: &[PathBuf]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW};
    use windows::core::PCWSTR;

    if files.is_empty() {
        return Ok(());
    }
    // pFrom 是一串以 \0 分隔、以两个 \0 结尾的完整路径
    let mut from: Vec<u16> = Vec::new();
    for f in files {
        let abs = std::path::absolute(f).map_err(|e| e.to_string())?;
        from.extend(abs.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted.as_bool() {
        return Err(format!("移到回收站失败（错误码 {code}），文件可能正被别的程序占用（播放器、正在放这场录像的游戏）"));
    }
    Ok(())
}

/// 用系统默认的程序打开文件（播放视频）
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    if !Path::new(&path).is_file() {
        return Err(format!("文件不在了：{path}"));
    }
    std::process::Command::new("explorer").arg(&path).spawn().map(drop).map_err(|e| e.to_string())
}
