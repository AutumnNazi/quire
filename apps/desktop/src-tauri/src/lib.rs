//! Quire 桌面端的核心逻辑。
//!
//! 代码全部放在 lib 里,集成测试才能直接用到这些模块;`main.rs` 只负责启动装配。
//!
//! ## 为什么没有本机 HTTP 服务
//!
//! 早期版本靠浏览器扩展把剪藏结果 POST 过来,所以在 localhost 上开了个 axum 端口。
//! 现在改走剪贴板,那个服务没有任何消费者了——留着就等于在你机器上常驻一个
//! 监听端口,给同浏览器的恶意网页留了个可攻击面。直接拆掉,比加防护干净。

pub mod assets;
pub mod clipboard;
pub mod frontmatter;
pub mod ids;
pub mod search;
pub mod slug;
pub mod vault;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use clipboard::ClipboardCapture;
use search::SearchHit;
use chrono::Local;
use tauri_plugin_dialog::DialogExt;
use vault::{ClipContent, ClipInput, ClipSummary, SavedClip, ScanResult, SharedVault, Vault};

/// 剪贴板轮询间隔。开启监控后一直在读剪贴板,太密会白耗 CPU,
/// 太疏则用户复制完要干等。
const WATCH_INTERVAL_MS: u64 = 700;

pub struct AppState {
    vault: SharedVault,
    watch: Arc<WatchState>,
}

#[derive(Default)]
struct WatchState {
    enabled: AtomicBool,
    /// 上一次广播出去的剪贴板内容指纹。轮询会反复读到同一份数据,
    /// 没有这个就会每 700ms 重复提示一次。
    last_hash: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultInfo {
    pub path: String,
    pub watching: bool,
}

/// 后台把图片下完之后的通知,让界面能告诉用户"图也存下来了"。
#[derive(Clone, Serialize)]
struct ImagesLocalized {
    filename: String,
    count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClipSavedNotice {
    pub id: String,
    pub filename: String,
}

fn current_vault(state: &State<AppState>) -> Result<Arc<Vault>, String> {
    // 锁中毒说明别的线程 panic 过,此时 vault 状态不可信,直接报错让用户重启
    state.vault.read().map(|g| g.clone()).map_err(|e| e.to_string())
}

fn info_of(state: &State<AppState>) -> VaultInfo {
    let path = current_vault(state)
        .map(|v| v.root().display().to_string())
        .unwrap_or_default();
    VaultInfo {
        path,
        watching: state.watch.enabled.load(Ordering::Relaxed),
    }
}

fn hash_of(capture: &ClipboardCapture) -> u64 {
    let mut hasher = DefaultHasher::new();
    capture.html.hash(&mut hasher);
    capture.text.hash(&mut hasher);
    hasher.finish()
}

#[tauri::command]
fn vault_info(state: State<AppState>) -> VaultInfo {
    info_of(&state)
}

#[tauri::command]
fn list_clips(state: State<AppState>) -> Result<ScanResult, String> {
    let vault = current_vault(&state)?;
    vault.scan().map_err(|e| e.to_string())
}

#[tauri::command]
fn read_clip(filename: String, state: State<AppState>) -> Result<ClipContent, String> {
    let vault = current_vault(&state)?;
    vault.read_clip(&filename).map_err(|e| e.to_string())
}

/// 全文检索。返回摘要 + 命中片段,前端直接拿去渲染列表。
#[tauri::command]
fn search_clips(
    query: String,
    limit: Option<usize>,
    state: State<AppState>,
) -> Result<Vec<SearchHit>, String> {
    let vault = current_vault(&state)?;
    // 上限是防手滑的闸,不是业务规则。一次要一万条,界面也渲染不动。
    let limit = limit.unwrap_or(200).min(1000);
    search::search(&vault, &query, limit).map_err(|e| e.to_string())
}

/// 改已读 / 归档标志。传 `None` 表示这一项不动。
#[tauri::command]
fn set_clip_flags(
    filename: String,
    read: Option<bool>,
    archived: Option<bool>,
    state: State<AppState>,
) -> Result<ClipSummary, String> {
    let vault = current_vault(&state)?;
    vault.set_flags(&filename, read, archived).map_err(|e| e.to_string())
}

/// 移进回收站。**不真删**——剪藏工具里唯一能把用户东西弄没的操作,
/// 没必要一按就没。真要清空,用户自己去 `clips/.trash/` 里翻。
#[tauri::command]
fn trash_clip(filename: String, state: State<AppState>) -> Result<(), String> {
    let vault = current_vault(&state)?;
    vault.trash(&filename).map_err(|e| e.to_string())
}

/// 从回收站放回原位。
#[tauri::command]
fn restore_clip(filename: String, state: State<AppState>) -> Result<ClipSummary, String> {
    let vault = current_vault(&state)?;
    vault.restore(&filename).map_err(|e| e.to_string())
}

/// 把整个剪藏库拼成单个 Markdown,写到用户选的位置。
///
/// 走对话框让用户自己定存哪、叫什么名——导出是用户的动作,
/// 替他在某个目录里造个文件等于替他做决定。
#[tauri::command]
fn export_vault(app: AppHandle, state: State<AppState>) -> Result<Option<String>, String> {
    let vault = current_vault(&state)?;
    let markdown = vault.export_markdown().map_err(|e| e.to_string())?;

    let picked = app
        .dialog()
        .file()
        .set_file_name(default_export_name())
        .add_filter("Markdown", &["md"])
        .blocking_save_file();
    let Some(picked) = picked else {
        return Ok(None); // 用户点了取消,不是故障
    };
    let Some(path) = picked.into_path().ok() else {
        return Err("选中的不是一个可写的文件位置".into());
    };
    std::fs::write(&path, markdown).map_err(|e| format!("写入失败: {e}"))?;
    Ok(Some(path.display().to_string()))
}

/// 导出文件名带上日期。同一周导两次不会互相覆盖,而用户回头翻的时候
/// 也知道是哪一份。
fn default_export_name() -> String {
    format!("Quire-剪藏导出-{}.md", Local::now().format("%Y-%m-%d"))
}

/// 读一次剪贴板。前端拿到 HTML 后转成 Markdown,再调 [`save_clip`] 落盘。
///
/// 拆成两步是因为 HTML→Markdown 需要 DOM,而 DOM 只存在于 webview 里。
#[tauri::command]
fn capture_clipboard() -> Result<ClipboardCapture, String> {
    clipboard::capture_clipboard()
}

#[tauri::command]
fn save_clip(
    app: AppHandle,
    input: ClipInput,
    state: State<AppState>,
) -> Result<SavedClip, String> {
    let vault = current_vault(&state)?;
    let saved = vault.save(&input).map_err(|e| e.to_string())?;
    let _ = app.emit(
        "clip-saved",
        ClipSavedNotice {
            id: saved.id.clone(),
            filename: saved.filename.clone(),
        },
    );
    spawn_image_localization(app, vault, saved.filename.clone());
    Ok(saved)
}

/// 后台把文章里的图片下到本地,下完再发一次 `clip-saved-images`。
///
/// **必须放后台。** 图片本地化可能要下几十兆、走好几秒,挡在保存流程前面
/// 的话,`Ctrl+V` 按下去要等三秒才看到东西——剪藏工具的全部意义就是那一下
/// 得是快的,为了几张图把它拖慢不划算。
fn spawn_image_localization(app: AppHandle, vault: Arc<Vault>, filename: String) {
    tauri::async_runtime::spawn_blocking(move || {
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Quire/0.1 (local-first read-later)")
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                // 建不出客户端就是没网之类的环境问题,静默跳过即可:
                // 图片没存下来不该挡住剪藏本身
                let _ = e;
                return;
            }
        };
        let result = vault.localize_images(&filename, |url| {
            let resp = client.get(url).send().map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("HTTP {}", resp.status()));
            }
            let ct = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let bytes = resp.bytes().map_err(|e| e.to_string())?.to_vec();
            Ok((ct, bytes))
        });
        if let Ok((saved, _)) = result {
            if saved > 0 {
                let _ = app.emit("clip-saved-images", ImagesLocalized { filename, count: saved });
            }
        }
    });
}

/// 开关剪贴板监控。默认关闭:被动监听会连你复制的密码、验证码、快递单号
/// 一起捕获,当默认行为太吵,得由用户自己决定要不要。
#[tauri::command]
fn set_clipboard_watch(enabled: bool, state: State<AppState>) -> Result<VaultInfo, String> {
    if enabled {
        // 开启的瞬间把剪贴板里现有的内容记成"已见"。否则用户刚打开开关,
        // 就会被自己几分钟前复制的东西弹一次提示,平白觉得这东西在窥探。
        if let Ok(capture) = clipboard::capture_clipboard() {
            state.watch.last_hash.store(hash_of(&capture), Ordering::Relaxed);
        }
    }
    state.watch.enabled.store(enabled, Ordering::Relaxed);
    Ok(info_of(&state))
}

/// 弹出目录选择器,选完立刻切换并广播。取消选择返回 None,保持原目录不变。
#[tauri::command]
async fn pick_vault(app: AppHandle, state: State<'_, AppState>) -> Result<Option<VaultInfo>, String> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });

    // recv 会一直阻塞到用户在对话框里做出选择,而这是个 async 命令,跑在 tokio 上。
    // 直接调会占死 worker 线程——单线程 runtime 下就是彻底死锁。
    // 挪进阻塞线程池去等,async 这边只管 await 一次。
    let received = tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|_| "选择器没有返回结果".to_string())?;

    let Some(picked) = received else {
        return Ok(None); // 用户取消,保持原目录
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;

    let vault = Vault::new(&path);
    vault.ensure_dirs().map_err(|e| e.to_string())?;
    {
        let mut guard = state.vault.write().map_err(|e| e.to_string())?;
        *guard = Arc::new(vault);
    }
    let info = info_of(&state);
    let _ = app.emit("vault-changed", &info);
    Ok(Some(info))
}

/// 在系统文件管理器里打开剪藏目录——这是"数据在你手上"最直观的一次兑现,
/// 用户随时能看见、随时能拷走。
#[tauri::command]
fn open_vault_folder(state: State<AppState>) -> Result<(), String> {
    let vault = current_vault(&state)?;
    vault.ensure_dirs().map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");

    cmd.arg(vault.root())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开剪藏目录失败: {e}"))
}

fn default_vault_dir() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Quire")
}

/// 后台轮询剪贴板。默认关闭,只把内容**通知**给前端,不自动保存——
/// 自动存等于替用户做决定,而且监控开着的时候什么都会往里灌。
fn start_clipboard_watch(app: AppHandle, state: Arc<WatchState>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(WATCH_INTERVAL_MS));
            if !state.enabled.load(Ordering::Relaxed) {
                continue;
            }
            let Ok(capture) = clipboard::capture_clipboard() else {
                continue;
            };
            if capture.is_empty() {
                continue;
            }
            let hash = hash_of(&capture);
            if state.last_hash.swap(hash, Ordering::Relaxed) == hash {
                continue;
            }
            let _ = app.emit("clipboard-changed", capture);
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let vault = Vault::new(default_vault_dir());
            // 目录建不出来也要把应用起起来——用户可能选了只读盘或网络盘,
            // 让他在界面上看到真实报错,好过窗口都开不出来
            let _ = vault.ensure_dirs();

            let watch = Arc::new(WatchState::default());
            start_clipboard_watch(handle, watch.clone());
            app.manage(AppState {
                vault: vault::shared(vault),
                watch,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            vault_info,
            list_clips,
            read_clip,
            search_clips,
            set_clip_flags,
            trash_clip,
            restore_clip,
            export_vault,
            capture_clipboard,
            save_clip,
            set_clipboard_watch,
            pick_vault,
            open_vault_folder
        ])
        .run(tauri::generate_context!())
        .expect("Quire 启动失败");
}
