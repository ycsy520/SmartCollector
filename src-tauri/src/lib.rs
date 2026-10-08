// 组装层。P8：invoke_handler 全量注册 + AppState（DB 池）注入 + 后台 worker +
// 系统外壳（托盘 + 关窗最小化到托盘、全局快捷键唤起、单实例、开机自启）。
// 剪贴板只在用户主动动作时读一次，无后台监听（见 clip::collect）。
mod agent;
mod clip;
mod commands;
mod config;
mod db;
mod dto;
mod error;
mod events;
mod llm;
mod services;
mod state;
mod vector;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

/// 显示并聚焦主窗口（单实例二次启动、托盘、全局快捷键共用）。
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// 托盘「收集剪贴板」：**用户主动点击**时才读一次系统剪贴板并收进缓冲区（02 §1.2）。
/// 应用不做任何后台轮询监听——复制过的内容不会被自动收走（2026-09-26 用户裁定的硬边界）。
/// 回执只走事件通道（`fragment://created`），与前端按钮调用 `submit_clipboard` 同一条路径。
fn collect_clipboard(app: &AppHandle) {
    use tauri::Emitter;
    let state = app.state::<state::AppState>();
    let Ok(conn) = state.db.get() else { return };
    let text = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok());
    if let Ok(dto::fragment::ClipboardOutcome::Collected(o)) =
        commands::fragment::submit_clipboard_impl(&conn, text.as_deref())
    {
        let _ = app.emit(
            events::FRAGMENT_CREATED,
            events::FragmentCreated { fragment_id: o.fragment_id, source: "clipboard" },
        );
    }
}

/// 全局加速键：唤起主窗口 + 把剪贴板文本填进输入框**草稿**。
/// 与托盘「收集剪贴板」的分工：那条是明示收集、立即落库；这条只交付草稿，
/// 不入库、不经模型，是否收集仍由用户按「收进来」决定（剪贴板红线只放宽了"读一次"，
/// 没有放宽"存一条"）。键位可配置（config.paste_shortcut，设置→系统）。
pub(crate) fn summon_with_clipboard_draft(app: &AppHandle) {
    use tauri::Emitter;
    show_main(app);
    let text = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok());
    let _ = app.emit(events::CLIPBOARD_DRAFT, events::ClipboardDraft { text });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 本地开发：把 .env 里的 DEEPSEEK_API_KEY / DASHSCOPE_API_KEY 等载入进程环境（不覆盖真实环境变量、不落库）。
    // 生产密钥应由 keyring 提供（P8），.env 仅为临时开发引导。
    config::secrets::load_dotenv();

    tauri::Builder::default()
        // 单实例：二次启动不再开新进程，转而唤起已有主窗口。须最先注册。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        // 开机自启：登录项由本插件管理，状态与 config.autostart 同步（见 commands::settings）。
        .plugin(tauri_plugin_autostart::init(MacosLauncher::AppleScript, None))
        // 目录选择器（批次22-A）：只给设置页「更改存放目录」用。选完仍要后端校验绝对路径 +
        // 搬图 + 放行 asset 作用域，插件本身不给前端任何读写能力。
        .plugin(tauri_plugin_dialog::init())
        // 全局快捷键：一枚可配的「收集加速键」（见 setup）——唤起主窗口 + 读一次剪贴板填草稿。
        // 键位语义仅此一条：开窗口另有托盘双击/任务栏，不再独占一枚"只唤窗"的系统级全局键。
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .on_window_event(|window, event| {
            // 关闭主窗口 = 隐藏到托盘（进程与后台 worker 继续），而非退出。
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            // 应用数据目录下的 smart.db；init_pool 会跑迁移并注册 sqlite-vec 扩展。
            let data_dir = app.path().app_data_dir()?;
            // 图片收藏目录的**默认值**（批次21-B）：与 tauri.conf.json 的 assetProtocol.scope
            // `$APPDATA/media/**` 同处，故前端 convertFileSrc 才读得到。批次22-A 起用户可改到别处，
            // 改出去的目录不在静态 scope 里，须由下面（启动）与 apply_media_dir（保存）运行期放行。
            services::media::set_default_dir(data_dir.join("media"));
            // 启动失败必须留下痕迹：release 配置是 `panic="abort"` + `strip`，返回 Err 时
            // 进程直接消失，用户看不到窗口、看不到日志、也复述不出症状。
            // 只用文件，不用 dialog 插件——插件的 `blocking_show()` 是 `show()` + `rx.recv()`，
            // 而 `show()` 靠 `run_on_main_thread` 投递；setup 正跑在主线程上，等自己 = 死锁。
            let pool = match db::init_pool(&data_dir.join("smart.db")) {
                Ok(p) => p,
                Err(e) => {
                    use std::io::Write;
                    let _ = std::fs::create_dir_all(&data_dir);
                    let log = data_dir.join("startup-error.log");
                    match std::fs::OpenOptions::new().create(true).append(true).open(&log) {
                        Ok(mut f) => {
                            // 只写确实成立的事实：快照未必存在（全新空库不备份、备份失败也照样启动），
                            // 在日志里断言"快照在 xxx"会误导救援。
                            let _ = writeln!(f, "[{}] 启动失败：{e}", db::now_iso());
                        }
                        Err(_) => eprintln!("[smart-collector] 无法写启动日志 {log:?}: {e}"),
                    }
                    eprintln!("[smart-collector] 启动失败: {e}");
                    return Err(e.into());
                }
            };
            app.manage(state::AppState { db: pool.clone() });
            let handle = app.handle().clone();
            // 后台 worker：独立 std 线程排空 fragment_status 队列，跑 AgentPipeline + 落向量 + 发事件。
            // （blocking reqwest 不能跑在 tauri 的 async 运行时线程上，见 agent::worker 头注释。）
            agent::worker::spawn(pool.clone(), handle.clone());
            // 剪贴板**不**做后台监听：收集只在用户主动动作时发生（输入框粘贴 submit_text、
            // 托盘「收集剪贴板」/前端按钮 submit_clipboard）。见 clip::collect 头注释。

            // 启动即把 OS 登录项与加速键对齐到 config（config 为唯一事实源）。
            let cfg = {
                let conn = pool.get().map_err(|e| e.to_string())?;
                db::config_store::load(&conn).unwrap_or_default()
            };
            {
                use tauri_plugin_autostart::ManagerExt;
                let al = handle.autolaunch();
                let _ = if cfg.autostart { al.enable() } else { al.disable() };
            }
            // 用户自定义的图片目录（批次22-A）：静态 scope 只覆盖默认目录，改过就要在运行期
            // 精确放行这一个目录（不放宽成 `**`——那等于把整块磁盘交给页面里的 <img>）。
            // 放行失败不阻断启动：设置页显示的是**生效**目录，用户看得见，也改得回来。
            if !cfg.media_dir.trim().is_empty() {
                if let Some(dir) = services::media::resolve_dir(&cfg.media_dir) {
                    services::media::set_media_dir(dir.clone());
                    if let Err(e) = handle.asset_protocol_scope().allow_directory(&dir, true) {
                        eprintln!("[smart-collector] 图片目录未开放（缩略图可能显示不出来）: {e}");
                    }
                }
            }

            // 收集加速键（可改键，设置→系统）：唤起窗口 + 读一次剪贴板填首页草稿。注册失败不阻断启动，
            // 但把原因记进运行时状态，设置页据此明示"键位存着但按了没反应"，用户可改键。
            if let Err(e) = commands::settings::apply_paste_shortcut(&handle, &cfg.paste_shortcut) {
                commands::settings::set_shortcut_error(Some(e.to_string()));
                eprintln!("[smart-collector] {e}");
            }

            // 系统托盘：右键菜单「收集剪贴板 / 显示主窗口 / 退出」。第一项是用户主动收集入口。
            let collect = MenuItem::with_id(app, "collect", "收集剪贴板", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&collect, &show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main-tray")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Smart Collector")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "collect" => collect_clipboard(app),
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                // 双击托盘图标唤起主窗口。**只认 DoubleClick**：Windows 的一次双击会连发
                // Click(Down) → Click(Up) → DoubleClick 三个事件（`tray-icon` 的窗口过程对
                // 每个消息无条件派发、不做去重），若 Click 分支也动手，双击就先唤一次再被
                // 第二个事件重复一次。Hover 类事件（Enter/Move/Leave）同理忽略。
                // 隐藏态与最小化态都由 `show_main` 归位；它在窗口已可见时是空操作
                // （tao 的 WindowFlags 差异早退），所以不存在"唤完又缩回去"。
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::fragment::submit_text,
            commands::fragment::submit_clipboard,
            commands::media::submit_image,
            commands::fragment::get_fragments,
            commands::fragment::get_fragment,
            commands::fragment::retry_fragment,
            commands::fragment::delete_fragment,
            commands::fragment::purge_fragment,
            commands::fragment::update_fragment,
            commands::fragment::set_fragment_layer,
            commands::fragment::run_skill,
            commands::search::search_fragments,
            commands::export::export_fragments,
            commands::curation::list_related,
            commands::curation::get_week_digest,
            commands::usage::get_usage_stats,
            commands::settings::get_config,
            commands::settings::update_config,
            commands::settings::test_llm_config,
            commands::curation::list_skills,
            commands::curation::save_skill,
            commands::curation::delete_skill,
            commands::curation::reset_skill,
            commands::curation::list_habits,
            commands::curation::set_habit_state,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // 退出前无额外清理：DB 走 WAL + 连接池随进程析构关闭；worker 线程随 main 退出。
            if let RunEvent::Exit = event {
                // no-op（保留分支以便后续接入优雅关闭钩子）
            }
        });
}
