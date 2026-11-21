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
    /// 游戏模式（DOTA_GameMode，22 = 全英雄选择，23 = 加速模式）。2026-11-16 起才记，之前的记录没有
    #[serde(default)]
    pub game_mode: Option<i32>,
    pub player: usize,
    /// 英雄内部名（不带前缀）
    pub hero: String,
    pub player_name: String,
    /// 录的是不是"我"（`account::me`）。已录视频页 2026-11-15 起不再显示，照样记着
    pub is_me: bool,
    /// 这次选了要录的片段
    pub clips: Vec<RunClip>,
    /// 合集文件；没拼合集为 None
    pub compilation: Option<PathBuf>,
    /// 合集是哪种集锦（`clips::Collection::key`）；2026-11-16 之前的记录没有，界面叫"合集"
    #[serde(default)]
    pub compilation_kind: Option<String>,
    /// 这次做成的成品（2026-11-18 起：「推荐」的集锦和单个时刻、「自定义集锦」）。之前的记录只有 `compilation`
    #[serde(default)]
    pub products: Vec<RunProduct>,
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
    /// 片段类别和特别场景（`clips::Kind::key`，残血反杀、抢盾）。2026-11-16 之前的记录没有，当成击杀片段
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub low_hp: bool,
    #[serde(default)]
    pub teamwork: bool,
    #[serde(default)]
    pub aegis_steal: bool,
    /// 录成的文件；没录完为 None
    pub path: Option<PathBuf>,
}

/// 一个成品
#[derive(Serialize, Deserialize, Clone)]
pub struct RunProduct {
    /// `clips::ProductKind::key`，「自定义」拼的是 "custom"
    pub kind: String,
    pub file: PathBuf,
    /// 用到的片段（`RunClip::file_name`）
    pub clips: Vec<String>,
    /// 只有一段的高光时刻的起止（游戏时钟）；集锦和多段的为 None
    pub start: Option<f32>,
    pub end: Option<f32>,
}

impl Run {
    /// 这次录成了视频（单段、集锦或成品）。都没有的不记进已录视频
    pub fn has_videos(&self) -> bool {
        self.clips.iter().any(|c| c.path.is_some()) || self.compilation.is_some() || !self.products.is_empty()
    }
}

fn path() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(|d| PathBuf::from(d).join(r"dota2-highlight-recorder\history.json"))
}

fn load() -> Vec<Run> {
    path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn store(runs: &[Run]) -> Result<(), String> {
    let p = path().ok_or_else(|| crate::uierr::key("err.noAppData"))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| crate::uierr::detail("err.saveHistory", e))?;
    }
    let text = serde_json::to_string_pretty(runs).map_err(|e| crate::uierr::detail("err.saveHistory", e))?;
    // 先写临时文件再改名，写到一半退出也不会把整个记录弄坏
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| crate::uierr::detail("err.saveHistory", format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &p).map_err(|e| crate::uierr::detail("err.saveHistory", format!("{}: {e}", p.display())))
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
    kind: Option<String>,
    low_hp: bool,
    teamwork: bool,
    aegis_steal: bool,
    /// 最近一次录成的文件；从没录成为 None（"未录完"）
    file: Option<FileRecord>,
    /// 拼进了还在的集锦或成品里（2026-11-18 起拼完就删单段，`file` 为 None 也不算没录完）
    in_product: bool,
}

#[derive(Serialize)]
pub struct PlayerRecord {
    player: usize,
    hero: String,
    player_name: String,
    /// 按开始时间排序
    clips: Vec<ClipRecord>,
    /// 拼过的集锦，最近拼的在前。不同种类文件名不同，各留一个；同一种重拼覆盖同一个文件
    compilations: Vec<CompilationRecord>,
}

/// 一个集锦或成品
#[derive(Serialize)]
pub struct CompilationRecord {
    /// 成品是 `clips::ProductKind::key` 或 "custom"；2026-11-18 之前拼的集锦是 `clips::Collection::key`；
    /// 2026-11-16 之前拼的没有，界面叫"合集"
    kind: Option<String>,
    file: FileRecord,
    /// 用了几段；以前的合集不知道，为 0
    clips: usize,
    /// 只有一段的高光时刻的起止；集锦和多段的为 None
    start: Option<f32>,
    end: Option<f32>,
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
    /// 游戏模式；记录里没有（2026-11-16 之前录的）就读本地录像，录像也没了为 None
    game_mode: Option<i32>,
    /// 录像还在本地（已录视频里没了的标「录像已删除」，2026-11-20 起）
    replay: bool,
    /// 最近一次录制的时间
    last_time: u64,
    players: Vec<PlayerRecord>,
    /// 最近的在前
    runs: Vec<RunSummary>,
}

/// 按比赛汇总的录制记录，最近录过的比赛在前
#[tauri::command]
pub fn recorded_matches() -> Vec<MatchRecord> {
    // 一个视频都没录成的旧记录（2026-11-20 前也记）跳过
    let runs: Vec<Run> = {
        let _g = LOCK.lock().unwrap();
        load().into_iter().filter(Run::has_videos).collect()
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
                    clips: Vec::new(),
                    compilations: Vec::new(),
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
                    kind: c.kind.clone(),
                    low_hp: c.low_hp,
                    teamwork: c.teamwork,
                    aegis_steal: c.aegis_steal,
                    file,
                    in_product: false,
                }),
            }
        }
        // 同一个文件重拼过：去掉旧的那条，新的放最前面
        let mut add = |c: CompilationRecord| {
            p.compilations.retain(|r| r.file.path != c.file.path);
            p.compilations.insert(0, c);
        };
        if let Some(c) = &run.compilation {
            add(CompilationRecord { kind: run.compilation_kind.clone(), file: FileRecord::new(c), clips: 0, start: None, end: None });
        }
        for x in run.products.iter().rev() {
            add(CompilationRecord { kind: Some(x.kind.clone()), file: FileRecord::new(&x.file), clips: x.clips.len(), start: x.start, end: x.end });
        }
    }
    let inside = in_products(runs);
    for p in &mut players {
        p.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
        for c in &mut p.clips {
            c.in_product = inside.contains(&c.file_name);
            // 单段文件拼进成品后就删了（2026-11-18 起）：以前停下时留着的单段，重新录拼完后文件没了，
            // 那条旧记录还指着它，会显示成「文件不在了」（2026-11-20）。拼在还在的成品里的，就当没有单段文件
            if c.in_product && c.file.as_ref().is_some_and(|f| !f.exists) {
                c.file = None;
            }
        }
    }
    // 按第一次录的先后（不再把"我"排前面：已录视频页不分你我，2026-11-15 用户定）
    let dem = game::launch::replays_dir().ok().map(|d| d.join(format!("{match_id}.dem")));
    let replay = dem.as_deref().is_some_and(Path::is_file);
    let game_mode = runs.iter().rev().find_map(|r| r.game_mode).or_else(|| {
        let dem = dem.as_deref().filter(|_| replay)?;
        replay::peek(dem).ok().map(|s| s.game_mode)
    });
    MatchRecord {
        match_id,
        game_mode,
        replay,
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

/// 拼进了还在的集锦或成品里的片段（`RunClip::file_name`）。2026-11-18 之前的合集里是那次录成的全部片段
fn in_products(runs: &[&Run]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for run in runs {
        if run.compilation.as_deref().is_some_and(Path::is_file) {
            out.extend(run.clips.iter().filter(|c| c.path.is_some()).map(|c| c.file_name.clone()));
        }
        for x in run.products.iter().filter(|x| x.file.is_file()) {
            out.extend(x.clips.iter().cloned());
        }
    }
    out
}

/// 这场比赛「推荐」做出来、文件还在的成品（比赛页标"已生成"用）。以前「拼成集锦」拼的击杀集锦和新的同名，
/// 但内容是当时勾的那几段，不算
pub fn product_files(match_id: u64) -> std::collections::HashSet<PathBuf> {
    let _g = LOCK.lock().unwrap();
    load()
        .into_iter()
        .filter(|r| r.match_id == match_id)
        .flat_map(|r| r.products)
        .filter(|x| x.kind != "custom" && x.file.is_file())
        .map(|x| x.file)
        .collect()
}

/// 删掉一场比赛的记录和视频：`player` 给了只删这个玩家（英雄）的（已录视频一个英雄一块，2026-11-19 起），没给删整场。
/// 视频文件移到回收站，返回移走了几个。原来可以只去掉记录、留着文件（确认框里的勾），2026-11-19 用户定不要这个选项
#[tauri::command]
pub fn delete_match_record(match_id: u64, player: Option<usize>) -> Result<usize, String> {
    let _g = LOCK.lock().unwrap();
    let mut runs = load();
    let (gone, keep): (Vec<Run>, Vec<Run>) =
        runs.drain(..).partition(|r| r.match_id == match_id && player.is_none_or(|p| r.player == p));
    let mut files: Vec<PathBuf> = gone
        .iter()
        .flat_map(|r| r.clips.iter().filter_map(|c| c.path.clone()).chain(r.compilation.clone()).chain(r.products.iter().map(|x| x.file.clone())))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files.dedup();
    recycle(&files)?;
    store(&keep)?;
    Ok(files.len())
}

/// 删一个视频（已录视频卡片上的垃圾桶，2026-11-19 起）：文件移到回收站，记录里去掉它。
/// - 成品或集锦：去掉这个成品。拼进它的单段（文件拼完已经删了）只在别的成品里没有时也去掉，
///   不然它们没文件、又不在成品里，会显示成「还有 N 个片段没录」；
/// - 单段：去掉这一段，当它没录过（不算没录完）；
/// - 一次录制什么都不剩了，这次的记录也去掉。
#[tauri::command]
pub fn delete_video(match_id: u64, player: usize, path: PathBuf) -> Result<(), String> {
    let _g = LOCK.lock().unwrap();
    let mut runs = load();
    if path.is_file() {
        recycle(std::slice::from_ref(&path))?;
    }
    let mine = |r: &Run| r.match_id == match_id && r.player == player;
    let mut orphaned = std::collections::HashSet::new();
    for r in runs.iter_mut().filter(|r| mine(r)) {
        if r.compilation.as_ref() == Some(&path) {
            r.compilation = None;
            r.compilation_kind = None;
        }
        r.products.retain(|x| {
            let hit = x.file == path;
            if hit {
                orphaned.extend(x.clips.iter().cloned());
            }
            !hit
        });
        r.clips.retain(|c| c.path.as_ref() != Some(&path));
    }
    let still: std::collections::HashSet<String> =
        runs.iter().filter(|r| mine(r)).flat_map(|r| r.products.iter().flat_map(|x| x.clips.iter().cloned())).collect();
    for r in runs.iter_mut().filter(|r| mine(r)) {
        // 文件已经不在的单段（以前停下时留的，后来拼进这个成品就删了）也算没文件
        r.clips.retain(|c| c.path.as_deref().is_some_and(Path::is_file) || !orphaned.contains(&c.file_name) || still.contains(&c.file_name));
    }
    runs.retain(|r| !(mine(r) && r.clips.is_empty() && r.products.is_empty() && r.compilation.is_none()));
    store(&runs)
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
        return Err(crate::uierr::with("err.recycle", serde_json::json!({ "code": code })));
    }
    Ok(())
}

/// 缩略图缓存目录：`%LOCALAPPDATA%\dota2-highlight-recorder\thumbs`
fn thumbs_dir() -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA").ok().map(|d| PathBuf::from(d).join(r"dota2-highlight-recorder\thumbs"))
}

/// 视频的缩略图（"已录视频"页用）：从第 4 秒（短视频取一半处）取一帧，400 像素宽。
/// 按路径、大小、修改时间缓存，视频重录了就重新取。返回 `data:image/jpeg;base64,…`，页面直接当图片地址用
#[tauri::command]
pub async fn video_thumb(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use std::hash::{Hash, Hasher};
        let video = PathBuf::from(&path);
        let meta = std::fs::metadata(&video).map_err(|_| crate::uierr::with("err.fileMissing", serde_json::json!({ "path": path })))?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&path, meta.len(), meta.modified().ok()).hash(&mut h);
        let dir = thumbs_dir().ok_or_else(|| crate::uierr::key("err.noAppData"))?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let jpg = dir.join(format!("{:016x}.jpg", h.finish()));
        if !jpg.is_file() {
            let settings = crate::settings::get_settings();
            let given = settings.ffmpeg.trim();
            let ff = media::ffmpeg::Ffmpeg::locate((!given.is_empty()).then(|| Path::new(given))).map_err(|e| format!("{e:#}"))?;
            let secs = media::mp4_duration(&video).unwrap_or(8.0) as f32;
            ff.thumbnail(&video, 4f32.min(secs / 2.0), 400, &jpg).map_err(|e| format!("{e:#}"))?;
        }
        let bytes = std::fs::read(&jpg).map_err(|e| e.to_string())?;
        Ok(format!("data:image/jpeg;base64,{}", base64(&bytes)))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 标准 base64（缩略图几十 KB，不值得为它加一个依赖）
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// 用系统默认的程序打开文件（播放器窗口里的「用系统播放器打开」）
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    if !Path::new(&path).is_file() {
        return Err(crate::uierr::with("err.fileMissing", serde_json::json!({ "path": path })));
    }
    std::process::Command::new("explorer").arg(&path).spawn().map(drop).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(name: &str, path: Option<&str>) -> RunClip {
        RunClip {
            index: 1,
            file_name: name.into(),
            start: 0.0,
            end: 5.0,
            victims: vec![],
            multi_kill: 0,
            kind: Some("kill".into()),
            low_hp: false,
            teamwork: false,
            aegis_steal: false,
            path: path.map(PathBuf::from),
        }
    }

    fn run(player: usize, clips: Vec<RunClip>, products: Vec<RunProduct>) -> Run {
        Run {
            time: 1,
            match_id: 7,
            game_mode: None,
            player,
            hero: "slark".into(),
            player_name: String::new(),
            is_me: false,
            clips,
            compilation: None,
            compilation_kind: None,
            products,
            cancelled: None,
            error: None,
            elapsed_secs: 1.0,
        }
    }

    fn product(file: &str, clips: &[&str]) -> RunProduct {
        RunProduct { kind: "kills".into(), file: file.into(), clips: clips.iter().map(|c| c.to_string()).collect(), start: None, end: None }
    }

    #[test]
    fn delete_one_video_keeps_the_rest() {
        // 假的 APPDATA，不碰真的录制记录；文件都不存在，不会去动回收站
        let dir = std::env::temp_dir().join(format!("d2hr-history-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("dota2-highlight-recorder")).unwrap();
        unsafe { std::env::set_var("APPDATA", &dir) };
        store(&[
            // 两个成品共用 b；c 只在要删的成品里；d 是没录成的
            run(0, vec![clip("a", None), clip("b", None), clip("c", None), clip("d", None)], vec![product("kills.mp4", &["a", "b", "c"]), product("triple.mp4", &["b"])]),
            run(0, vec![clip("e", Some("e.mp4"))], vec![]),
            run(1, vec![clip("a", None)], vec![product("other.mp4", &["a"])]),
        ])
        .unwrap();

        delete_video(7, 0, "kills.mp4".into()).unwrap();
        let runs = load();
        let names = |r: &Run| r.clips.iter().map(|c| c.file_name.clone()).collect::<Vec<_>>();
        // a、c 跟着成品去掉；b 还在 triple 里；d 没录成，留着让用户接着录
        assert_eq!(names(&runs[0]), ["b", "d"]);
        assert_eq!(runs[0].products.len(), 1);
        // 别的玩家不受影响
        assert_eq!(names(&runs[2]), ["a"]);

        // 删单段：这次录制什么都不剩了，整条记录也去掉
        delete_video(7, 0, "e.mp4".into()).unwrap();
        let runs = load();
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|r| r.clips.iter().all(|c| c.file_name != "e")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
