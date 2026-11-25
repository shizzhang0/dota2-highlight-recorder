//! 在应用里录制：把 `pipeline` 的进度事件转成界面事件，确认请求弹给界面等用户回答。
//!
//! 事件：`record-event`（`pipeline::Event`）、`record-confirm`（{id, question}）、`record-finished`（`Finished`）、
//! `record-stopping`（停下的原因）、`record-blocked`（拦下了几次操作）。一次只能录一场；录制在单独的线程里跑，命令立刻返回。
//!
//! 录制时 Dota 在最前面，挡住主窗口：Dota 画面左上角盖一个置顶的小进度窗（`overlay` 窗口），带「停止录制」按钮；
//! 主窗口照样显示进度，录完或出错弹系统通知。发给 Dota 的点击和按键被拦下（`game::input_guard`），不暂停。
//! 要用户拍板的事（要不要重启游戏）弹一个置顶的小对话框（`prompt` 窗口）；主窗口上同样能回答，哪边先回答都行，另一边跟着收起。

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};

use game::cancel::{self, Reason};
use media::ffmpeg::Ffmpeg;
use pipeline::{Camera, Event, Host, Options, Selected};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::settings::Settings;

static RUNNING: AtomicBool = AtomicBool::new(false);
/// 上次录完后游戏留着没退
static GAME_OPEN: AtomicBool = AtomicBool::new(false);
static NEXT_QUESTION: AtomicU64 = AtomicU64::new(1);
/// 上一次录制的结果（`record_result`）：界面没收到 `record-finished` 时自己来问。开始新的一次时清掉
static LAST_RESULT: Mutex<Option<Finished>> = Mutex::new(None);
/// 正在等用户回答的确认请求
static PENDING: Mutex<Option<(u64, Sender<bool>)>> = Mutex::new(None);

struct AppHost {
    app: AppHandle,
}

#[derive(Serialize, Clone)]
struct Question {
    id: u64,
    question: String,
}

impl Host for AppHost {
    fn event(&self, e: Event) {
        let _ = self.app.emit("record-event", e);
    }

    fn confirm(&self, ask: &pipeline::Ask) -> bool {
        let id = NEXT_QUESTION.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        *PENDING.lock().unwrap() = Some((id, tx));
        // 界面按语言翻译（ui/js/i18n.js 的 uiText）；原句只进日志
        let question = if ask.params.is_null() { crate::uierr::key(ask.key) } else { crate::uierr::with(ask.key, ask.params.clone()) };
        let q = Question { id, question };
        let _ = self.app.emit("record-confirm", q.clone());
        show_prompt(&self.app, Prompt::Confirm(q));
        rx.recv().unwrap_or(false)
    }
}

/// 置顶小对话框里显示什么
#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Prompt {
    /// 录制流程要用户回答是/否
    Confirm(Question),
}

fn show_prompt(app: &AppHandle, p: Prompt) {
    let Some(w) = app.get_webview_window("prompt") else { return };
    let _ = app.emit_to("prompt", "prompt-show", p);
    let _ = w.center();
    let _ = w.show();
    let _ = w.set_focus();
}

fn hide_prompt(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("prompt") {
        let _ = w.hide();
    }
}

/// 进度窗离 Dota 画面左上角多远（物理像素）
const OVERLAY_MARGIN: i32 = 16;

/// 这次一共录了不到这么多秒画面，就不记录制速度（2026-11-18 一次只录 6.9 秒的测出 0.17，平时 0.8 左右）
const MIN_SPEED_SECS: f32 = 15.0;

/// 把进度窗放到 Dota 画面左上角；只在 Dota（或进度窗自己，比如点了停止）在前台时显示，用户切到别的程序就收起——
/// 它是置顶窗口，不收的话会一直浮在用户正在用的程序上面（2026-11-10 用户反馈）。
/// `at` 是现在显示的位置（None = 收着），没变就不动。显示时不激活（SW_SHOWNOACTIVATE）：Dota 在前台渲染快一倍，别把前台抢过来。
/// 显示、隐藏都直接调系统接口：Tauri 记着窗口"没显示"（显示不是它做的），再让它 hide 它就什么都不做（2026-11-10 实测收不起来）
fn place_overlay(app: &AppHandle, at: &mut Option<(i32, i32)>) {
    let Some(w) = app.get_webview_window("overlay") else { return };
    let Ok(me) = w.hwnd() else { return };
    let fg = game::ui::foreground();
    let wanted = game::window::dota_window()
        .ok()
        .filter(|d| fg == d.0 as isize || fg == me.0 as isize)
        .and_then(game::window::client_rect)
        .map(|(x, y, _, _)| (x + OVERLAY_MARGIN, y + OVERLAY_MARGIN));
    if *at == wanted {
        return;
    }
    match wanted {
        Some(pos) => {
            let _ = w.set_position(tauri::PhysicalPosition::new(pos.0, pos.1));
            if at.is_none() {
                game::window::restore(windows::Win32::Foundation::HWND(me.0));
            }
        }
        None => game::window::hide(windows::Win32::Foundation::HWND(me.0)),
    }
    *at = wanted;
}

fn hide_overlay(app: &AppHandle) {
    if let Some(Ok(h)) = app.get_webview_window("overlay").map(|w| w.hwnd()) {
        game::window::hide(windows::Win32::Foundation::HWND(h.0));
    }
}

/// 回答确认请求（主窗口或置顶对话框）
#[tauri::command]
pub fn answer_confirm(app: AppHandle, id: u64, yes: bool) {
    let mut pending = PENDING.lock().unwrap();
    if pending.as_ref().is_some_and(|(i, _)| *i == id)
        && let Some((_, tx)) = pending.take()
    {
        let _ = tx.send(yes);
        hide_prompt(&app);
        // 两边都弹了，告诉另一边收起
        let _ = app.emit("record-confirm-done", id);
    }
}

/// 开始录制时发给界面
#[derive(Serialize, Clone)]
struct Started {
    clips: usize,
    /// 预计总耗时（秒，不含启动游戏），按这台机器实测的录制速度估
    estimated_secs: f32,
}

/// 录完或出错时弹系统通知（用户可能在用别的程序）。`lang` 是界面语言
fn notify(app: &AppHandle, f: &Finished, lang: &str) {
    let en = lang.starts_with("en");
    let (title, body) = match (&f.error, &f.cancelled) {
        // 出错原因是中文原文（来自录制流程），英文界面只说去应用里看
        (Some(_), _) if en => ("Recording failed".to_owned(), "Open the app to see what went wrong.".to_owned()),
        (Some(e), _) => ("录制出错".to_owned(), e.clone()),
        (None, Some(reason)) => (
            if en { "Recording stopped" } else { "录制已停止" }.to_owned(),
            if en { format!("{} of {} clips done, stopped by you", f.completed.len(), f.selected.len()) } else { format!("已完成 {}/{} 个片段（{reason}）", f.completed.len(), f.selected.len()) },
        ),
        (None, None) => {
            let mins = f.elapsed_secs / 60.0;
            let body = match (en, f.products) {
                (true, 0) => format!("{} clips, {mins:.1} min", f.completed.len()),
                (true, n) => format!("{n} videos, {mins:.1} min"),
                (false, 0) => format!("{} 个片段，用时 {mins:.1} 分钟", f.completed.len()),
                (false, n) => format!("生成 {n} 个视频，用时 {mins:.1} 分钟"),
            };
            (if en { "Recording finished" } else { "录制完成" }.to_owned(), body)
        }
    };
    let _ = app.notification().builder().title(title).body(body).show();
}

/// 录制结束（录完、停下或出错）时发给界面
#[derive(Serialize, Clone)]
pub struct Finished {
    match_id: u64,
    /// 写录制记录失败的原因（界面提示一下，不影响已经录好的文件）
    history_error: Option<String>,
    files: Vec<PathBuf>,
    /// 做好的片段序号（计划里的序号）
    completed: Vec<usize>,
    /// 这次选了要录的片段序号
    selected: Vec<usize>,
    /// 停下的原因（文字）；正常录完或出错是 null
    cancelled: Option<String>,
    error: Option<String>,
    /// 认得出的失败种类（`pipeline::fail_kind`，语言文件里的 `rec.fail.{种类}`）；认不出为 None，界面说"录制出错"再附原文
    error_kind: Option<&'static str>,
    elapsed_secs: f32,
    out_dir: String,
    /// 录制用的游戏还开着（停下或出错时保留；全部录完会退出，退出失败时也是 true）
    game_open: bool,
    /// 这次实测的录制速度（每秒墙钟录多少秒画面）
    speed: Option<f32>,
    /// 做好了几个成品（集锦、单个时刻、自定义集锦）；0 = 只有单段
    products: usize,
}

fn options(s: &Settings, outputs: Vec<pipeline::Output>) -> Options {
    Options {
        outputs,
        // 拼完就删单段（2026-11-18 用户定）；没有成品（「自定义」只勾一段、没勾拼成集锦）时单段就是结果，不删
        keep_parts: false,
        resolution: s.capture_size(),
        out: PathBuf::from(&s.out_dir),
        gsi_port: game::gsi::DEFAULT_PORT,
        // 全部录完一定退出游戏；中途停下或出错时保留，方便接着录。原先"跟游戏窗口一致"时设置里可以关，
        // 2026-11-17 用户要去掉这项设置（旧设置文件里的 quit_when_done 读的时候忽略）
        quit: true,
        camera: if s.camera == "player_view" { Camera::PlayerView } else { Camera::HeroChase },
        block_input: true,
        // 录完不切前台：Dota 退出后应用窗口自己回到前台，再切会让页面收不到「录完了」（见 `Options::return_focus`）
        return_focus: false,
    }
}

/// 一个成品在录制记录里要记的东西：种类、起止（单个时刻）
struct ProductMeta {
    kind: String,
    start: Option<f32>,
    end: Option<f32>,
}

/// 片头卡片上除了视频类型以外的内容（`media::intro`，2026-11-24 定）：英雄显示名、战绩、安装包里的立绘、
/// 录的这名玩家的名字。2026-11-24 用户改定：录自己和录别人一样，都显示玩家名、都不带头像
/// （原来只在录自己时放名字和头像）。画面里也显示玩家名（2026-11-25 起不开匿名模式）
fn intro_card(app: &AppHandle, r: &replay::Replay, player: usize, n: &clips::Naming) -> media::intro::Card {
    let p = &r.players[player];
    let id = p.hero.trim_start_matches("npc_dota_hero_");
    let (k, d, a) = crate::matches::kda(r, player);
    media::intro::Card {
        kind: String::new(),
        hero: n.heroes.get(id).cloned().unwrap_or_else(|| id.to_owned()),
        kda: format!("{k} / {d} / {a}"),
        player: Some(p.name.clone()).filter(|s| !s.trim().is_empty()),
        render: app.asset_resolver().get(format!("img/heroes/renders/{id}.webp")).map(|x| x.bytes),
    }
}

/// 开始录制。两种（2026-11-18 起）：
/// - `products` 不空：比赛页「推荐」选的成品（`clips::Product::id`），要录的段是它们用到的片段合起来，同一段只录一次；
/// - 否则录「自定义」里勾的 `clips`（计划里的序号），`compile` 时两段以上拼成「自定义集锦」（勾没勾存进设置）。
///
/// `camera` 是比赛页上选的镜头（存进设置），`lang` 是界面语言（通知和文件名用）。其他设置读当前保存的
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_recording(
    app: AppHandle,
    match_id: u64,
    player: usize,
    clips: Vec<usize>,
    products: Vec<String>,
    camera: String,
    compile: bool,
    lang: String,
) -> Result<(), String> {
    if clips.is_empty() && products.is_empty() {
        return Err(crate::uierr::key("err.noClips"));
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err(crate::uierr::key("err.alreadyRecording"));
    }
    *LAST_RESULT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let custom = products.is_empty();
    let prepared = async {
        let mut settings = crate::settings::get_settings();
        if settings.camera != camera || (custom && settings.compile != compile) {
            settings.camera = camera;
            if custom {
                settings.compile = compile;
            }
            crate::settings::save_settings(settings.clone())?;
        }
        // ffmpeg 随安装包附带，设置页不再让用户填（2026-11-11 用户定：普通用户不会关心）。
        // settings.json 里的 ffmpeg 字段和环境变量 D2HR_FFMPEG 还认，只给开发用
        let ffmpeg = settings.ffmpeg.trim();
        let ff = Ffmpeg::locate((!ffmpeg.is_empty()).then(|| std::path::Path::new(ffmpeg)))
            .map_err(|_| crate::uierr::key("err.noFfmpeg"))?;
        let r = crate::matches::parsed(match_id).await?;
        let p = settings.params();
        let plan = clips::plan(&r, player, &p);
        // 文件名跟着界面语言（2026-11-22 起）
        let n = crate::matches::naming(&lang);
        let pick = |i: usize, clip: &clips::Clip| Selected { index: i, clip: clip.clone(), name: clips::file_name(&r, clip, &n) };
        let card = intro_card(&app, &r, player, &n);
        let intro = |kind: &str| Some(media::intro::Card { kind: kind.replace('_', " "), ..card.clone() });
        let (selected, outputs, metas) = if custom {
            let selected: Vec<Selected> = clips
                .iter()
                .map(|&i| plan.get(i.wrapping_sub(1)).map(|c| pick(i, c)).ok_or_else(|| crate::uierr::with("err.noClip", serde_json::json!({ "n": i }))))
                .collect::<Result<_, _>>()?;
            let (outputs, metas) = if compile && selected.len() > 1 {
                let name = crate::history::free_custom_name(std::path::Path::new(&settings.out_dir), &clips::custom_file_name(&r, player, &n));
                let label = match n.lang {
                    clips::Lang::Zh => "自定义集锦",
                    clips::Lang::En => "Custom highlights",
                };
                let parts = (0..selected.len()).map(pipeline::Part::whole).collect();
                (vec![pipeline::Output { name, parts, intro: intro(label) }], vec![ProductMeta { kind: "custom".into(), start: None, end: None }])
            } else {
                (Vec::new(), Vec::new())
            };
            (selected, outputs, metas)
        } else {
            let all = clips::products(&r, &plan);
            let chosen: Vec<&clips::Product> = products
                .iter()
                .map(|id| all.iter().find(|x| x.id() == *id).ok_or_else(|| crate::uierr::with("err.noProduct", serde_json::json!({ "id": id }))))
                .collect::<Result<_, _>>()?;
            // 用到的片段合起来按时间排，共用的只录一次
            let mut used: Vec<usize> = chosen.iter().flat_map(|x| x.clips.iter().copied()).collect();
            used.sort_unstable();
            used.dedup();
            let selected = used.iter().map(|&i| pick(i + 1, &plan[i])).collect();
            let outputs = chosen
                .iter()
                .map(|x| pipeline::Output {
                    name: x.file_name(&r, &plan, &n),
                    parts: x
                        .clips
                        .iter()
                        .map(|&c| pipeline::Part { index: used.iter().position(|&u| u == c).expect("在 used 里"), range: x.part(&plan, c) })
                        .collect(),
                    intro: intro(x.kind.file_label(n.lang)),
                })
                .collect();
            let metas = chosen
                .iter()
                .map(|x| {
                    let one = (!x.kind.is_collection() && x.clips.len() == 1).then(|| &plan[x.clips[0]]);
                    ProductMeta { kind: x.kind.key().into(), start: one.map(|c| c.start()), end: one.map(|c| c.end()) }
                })
                .collect();
            (selected, outputs, metas)
        };
        let o = options(&settings, outputs);
        Ok::<_, String>((o, ff, r, p, selected, metas))
    }
    .await;
    let (o, ff, r, p, selected, metas) = match prepared {
        Ok(v) => v,
        Err(e) => {
            RUNNING.store(false, Ordering::SeqCst);
            pipeline::applog::write("录制", &format!("没能开始：比赛 {match_id}，{e}"));
            return Err(e);
        }
    };

    cancel::reset();
    let estimated_secs: f32 = selected.iter().map(|s| s.clip.estimated_seconds(&p)).sum();
    let _ = app.emit("record-started", Started { clips: selected.len(), estimated_secs });
    // 录制期间：进度窗跟着 Dota 窗口走（游戏一启动就出现）；点了停止就告诉界面"正在停下"（恢复设置要几秒）；
    // 拦下了用户的操作就告诉进度窗，让用户知道"点了没用、也不影响录制"
    let watcher = app.clone();
    std::thread::spawn(move || {
        let (mut stopping, mut blocked, mut overlay_at) = (false, 0, None);
        while RUNNING.load(Ordering::SeqCst) {
            if !stopping && let Some(reason) = cancel::pending() {
                stopping = true;
                let _ = watcher.emit("record-stopping", reason.describe());
                hide_prompt(&watcher);
            }
            let now = game::input_guard::blocked();
            if now != blocked {
                blocked = now;
                let _ = watcher.emit("record-blocked", now);
            }
            place_overlay(&watcher, &mut overlay_at);
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    });
    std::thread::spawn(move || {
        let host = AppHost { app: app.clone() };
        let time = crate::matches::now();
        let report = pipeline::run(&host, &r, player, &selected, &ff, &o);
        // 记下这台机器的录制速度，下次估耗时用（明显不合理的不要）
        // 录得太少（不到 `MIN_SPEED_SECS` 秒画面）时不记：只录几秒的话，一点卡顿就让速度差很多
        if let Some(speed) = report.speed.filter(|s| (0.05..5.0).contains(s) && report.recorded_secs >= MIN_SPEED_SECS) {
            let mut settings = crate::settings::get_settings();
            settings.add_speed(speed);
            let _ = crate::settings::save_settings(settings);
        }
        // 记进"已录视频"：files 和 completed 一一对应；拼完删掉的单段不记文件（拼在成品里，见 products）
        let done: std::collections::HashMap<usize, &PathBuf> =
            report.completed.iter().copied().zip(&report.files).filter(|(_, f)| !report.removed.contains(f)).collect();
        let hero = |i: usize| r.players[i].hero.trim_start_matches("npc_dota_hero_").to_owned();
        let products: Vec<crate::history::RunProduct> = report
            .products
            .iter()
            .map(|(k, file)| crate::history::RunProduct {
                kind: metas[*k].kind.clone(),
                file: file.clone(),
                clips: o.outputs[*k].parts.iter().map(|p| selected[p.index].name.clone()).collect(),
                start: metas[*k].start,
                end: metas[*k].end,
            })
            .collect();
        let run = crate::history::Run {
            time,
            match_id,
            game_mode: Some(r.game_mode),
            player,
            hero: hero(player),
            player_name: r.players[player].name.clone(),
            is_me: crate::account::me() == Some(r.players[player].account_id),
            clips: selected
                .iter()
                .map(|s| crate::history::RunClip {
                    index: s.index,
                    file_name: s.name.clone(),
                    start: s.clip.start(),
                    end: s.clip.end(),
                    victims: if s.clip.kind == clips::Kind::Kill { &s.clip.kills } else { &s.clip.assists }
                        .iter()
                        .map(|&k| hero(r.kills[k].victim))
                        .collect(),
                    multi_kill: s.clip.multi_kill,
                    kind: Some(s.clip.kind.key().to_owned()),
                    low_hp: s.clip.low_hp,
                    teamwork: s.clip.teamwork,
                    aegis_steal: s.clip.aegis_steal,
                    path: done.get(&s.index).map(|p| (*p).clone()),
                })
                .collect(),
            // 2026-11-18 起集锦都记在 products 里
            compilation: None,
            compilation_kind: None,
            products,
            cancelled: report.cancelled.map(|c| c.describe().to_owned()),
            error: report.error.as_ref().map(|e| format!("{e:#}")),
            elapsed_secs: report.elapsed.as_secs_f32(),
        };
        // 一个视频都没录成（启动中途停了、第一段就出错）不记：已录视频只放录好的视频（2026-11-20 用户定）。
        // 结果在录制页和通知里照样说，要再录点录制页上的「重新录」
        let history_error = if run.has_videos() { crate::history::add(run).err() } else { None };
        if let Some(e) = &history_error {
            pipeline::applog::write("录制", &format!("记进已录视频失败：{e}"));
        }
        let game_open = !report.game_quit && game::window::dota_window().is_ok();
        GAME_OPEN.store(game_open, Ordering::SeqCst);
        let finished = Finished {
            match_id,
            history_error,
            files: report.files,
            completed: report.completed,
            selected: selected.iter().map(|s| s.index).collect(),
            products: report.products.len(),
            cancelled: report.cancelled.map(|c| c.describe().to_owned()),
            error_kind: report.error.as_ref().and_then(pipeline::fail_kind),
            error: report.error.map(|e| format!("{e:#}")),
            elapsed_secs: report.elapsed.as_secs_f32(),
            out_dir: o.out.to_string_lossy().into_owned(),
            game_open,
            speed: report.speed,
        };
        *LAST_RESULT.lock().unwrap_or_else(|e| e.into_inner()) = Some(finished.clone());
        RUNNING.store(false, Ordering::SeqCst);
        // 主窗口显示这场比赛的录制详情；不抢前台（用户可能在用别的程序），点通知或任务栏图标就能看到。
        // 先发给界面，再收进度窗、弹通知：2026-11-18 录完后历史记录写好了，界面却一直停在「录制中」，
        // 不知道卡在哪一步，所以界面最要紧的这一步放最前面，前后都记进日志
        pipeline::applog::write("录制", &format!("告诉界面录完了：比赛 {match_id}"));
        let _ = app.emit("record-finished", finished.clone());
        // 进度窗录完立刻收起：结果在主窗口和系统通知里（留几秒的话会浮在用户正在用的程序上，2026-11-10 用户定）
        hide_overlay(&app);
        notify(&app, &finished, &lang);
        pipeline::applog::write("录制", "收起进度窗、弹完通知");
    });
    Ok(())
}

/// 让录制用的游戏退出（导航栏的"退出"）
#[tauri::command]
pub fn quit_game() -> Result<(), String> {
    game::netcon::Netcon::connect(2121).and_then(|mut c| c.send("quit")).map_err(|e| format!("{e:#}"))?;
    GAME_OPEN.store(false, Ordering::SeqCst);
    Ok(())
}

/// 录制用的游戏还开着（录完留着的那个）。用户自己关掉游戏后变 false。导航栏定时问
#[tauri::command]
pub fn game_open() -> bool {
    if GAME_OPEN.load(Ordering::SeqCst) && game::window::dota_window().is_err() {
        GAME_OPEN.store(false, Ordering::SeqCst);
    }
    GAME_OPEN.load(Ordering::SeqCst)
}

/// 点了停止（进度窗或主窗口上）
#[tauri::command]
pub fn stop_recording(app: AppHandle) {
    cancel::request(Reason::Stop);
    hide_prompt(&app);
    // 正在等确认的话当作"不同意"
    if let Some((_, tx)) = PENDING.lock().unwrap().take() {
        let _ = tx.send(false);
    }
}

#[tauri::command]
pub fn is_recording() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// 上一次录制的结果；还在录或者这次启动后没录过为 None。界面录完没收到 `record-finished` 时用（record.js）
#[tauri::command]
pub fn record_result() -> Option<Finished> {
    LAST_RESULT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
