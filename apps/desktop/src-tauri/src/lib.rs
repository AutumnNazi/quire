//! Quire 桌面端的核心逻辑。
//!
//! 代码全部放在 lib 里,集成测试才能直接用到这些模块;`main.rs` 只负责启动装配。
//!
//! ## 为什么没有本机 HTTP 服务
//!
//! 早期版本靠浏览器扩展把剪藏结果 POST 过来,所以在 localhost 上开了个 axum 端口。
//! 现在改走剪贴板,那个服务没有任何消费者了——留着就等于在你机器上常驻一个
//! 监听端口,给同浏览器的恶意网页留了个可攻击面。直接拆掉,比加防护干净。

pub mod clipboard;
pub mod frontmatter;
pub mod ids;
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
use vault::{ClipInput, ClipContent, SavedClip, ScanResult, SharedVault, Vault};

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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipSavedNotice {
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
    Ok(saved)
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
            capture_clipboard,
            save_clip,
            set_clipboard_watch,
            pick_vault,
            open_vault_folder
        ])
        .run(tauri::generate_context!())
        .expect("Quire 启动失败");
}
