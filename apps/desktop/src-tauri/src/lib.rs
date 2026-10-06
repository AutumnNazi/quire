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
#[cfg(windows)]
pub mod clipwatch;
pub mod fetch;
pub mod frontmatter;
pub mod ids;
pub mod import;
#[cfg(test)]
pub mod perfmeasure;
pub mod index;
pub mod search;
pub mod settings;
pub mod slug;
pub mod vault;
pub mod watcher;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use chrono::Local;
use clipboard::ClipboardCapture;
use search::SearchOutcome;
use vault::TagRenameReport;
use tauri_plugin_dialog::DialogExt;
use vault::{
    BatchReport, ClipContent, ClipInput, ClipSummary, ImportReport, RestoredBatch, SaveOutcome,
    ScanResult, SharedVault, TagCount, TrashListing, Vault, VaultError, WireError,
};

/// 剪贴板轮询间隔。开启监控后一直在读剪贴板,太密会白耗 CPU,
/// 太疏则用户复制完要干等。
const WATCH_INTERVAL_MS: u64 = 700;

/// 抓文章页面时用的 User-Agent。
///
/// **写明自己是谁,别装成浏览器。** 一堆站点按 UA 决定给不给全文,
/// 而伪装成 Chrome 拿到一篇它以为会给浏览器的内容,并不比老实说
/// 「我是个剪藏工具」更好——后者被拒了用户知道为什么,前者被拒了他不知道
pub const USER_AGENT: &str = concat!("Quire/", env!("CARGO_PKG_VERSION"), " (local read-later)");

pub struct AppState {
    vault: SharedVault,
    watch: Arc<WatchState>,
    /// 配置文件所在目录(不是配置文件本身)。存下来是为了改设置时能写回去
    config_dir: PathBuf,
    /// 上次记下的剪藏库目录,启动时**它已经不在了**。
    ///
    /// 这个状态只存在于"配置里指着一个地方,而那个地方没了"这一种情况。
    /// 不单独记着它的话,`ensure_dirs` 会把那个目录**重新建出来**,
    /// 于是用户看到的是一个干干净净的空库——而他的剪藏好好地躺在别处,
    /// 硬盘掉了、网盘没挂、同步盘换了路径,都可能造成这个。
    /// 界面上必须说"上次那个目录不在了",而不是假装一切正常
    ///
    /// 用 Mutex 是因为 Tauri 给的 `State` 是只读的,改不了普通字段。
    /// 中毒(别的线程 panic 过)按仓库既有做法报错,不 unwrap
    missing_vault: Mutex<Option<String>>,
    /// 全文检索索引。**键是目录**——剪藏库和回收站是两个目录,各有一份。
    /// 塞进同一份索引的话,同名文件(用户手工拷进去的)会互相顶掉,
    /// 表现是"在剪藏里搜到的那篇,点进去却是回收站里那篇"
    ///
    /// 索引文件本身**不放剪藏库里**。剪藏库多半开着网盘同步,
    /// 而 SQLite 塞进同步目录是冲突重灾区:两边各改一份,
    /// 下次同步就是两个文件打架,然后索引直接坏掉
    search_index: Mutex<HashMap<PathBuf, crate::index::Index>>,
}

/// 索引文件名。**把完整路径压成一个十六进制串**,而不是把路径本身当文件名
///
/// 不用路径当文件名:Windows 路径里的 `\`、`:`、中文都得转义,转出来的
/// 名字长得没法看,还可能撞车。压成哈希就没这些问题,而且切回旧剪藏库时
/// 那个索引还能接着用,不用从头建一遍
fn index_file_name(dir: &Path) -> String {
    // FNV-1a 64。手写而不是用 `DefaultHasher`:后者**不保证跨版本稳定**,
    // 哪天 Rust 换了算法,用户已有的索引就集体变成"陌生文件",
    // 全部重建——慢不说,还会让人以为是软件出了 bug
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in dir.to_string_lossy().as_bytes() {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}.sqlite")
}

/// 拿这个目录的索引,没有就建一份
fn index_for<'a>(
    state: &'a AppState,
    dir: &Path,
) -> Result<std::sync::MutexGuard<'a, HashMap<PathBuf, crate::index::Index>>, WireError> {
    let mut map = state
        .search_index
        .lock()
        .map_err(|_| WireError::new("vault.poisoned"))?;
    if !map.contains_key(dir) {
        let path = state.config_dir.join("indices").join(index_file_name(dir));
        map.insert(dir.to_path_buf(), crate::index::Index::open_or_recreate(&path));
    }
    Ok(map)
}

#[derive(Default)]
struct WatchState {
    enabled: AtomicBool,
    /// 上一次广播出去的剪贴板内容指纹。轮询会反复读到同一份数据,
    /// 没有这个就会每 700ms 重复提示一次。
    last_hash: AtomicU64,
    /// 连续读不到剪贴板的次数。**开关开着 ≠ 读得到东西**:别的程序长时间
    /// 占着剪贴板、剪贴板守护进程挂了、Wayland 下压根不支持——轮询会永远
    /// 失败下去,而开关在界面上始终显示已开启。用户复制十篇,Quire 一次
    /// 不响,他不会怀疑监控,只会怀疑自己复制的东西有问题,然后默默关掉开关。
    /// 所以得有个数记着"已经连续读不到多少下了"
    read_failures: AtomicU32,

}

/// 连续读不到多少次之后,告诉用户"监控其实没在工作"。
///
/// 一次失败不算数:别的程序偶尔占一下剪贴板是常事,那时候报"坏了"是在
/// 打扰一个没出问题的用户。**10 次**这个数不换算成时间——换了平台就不准:
/// Windows 上是事件驱动(系统一改就通知,没有固定周期),别的平台是 700ms
/// 轮询(10 次约 7 秒),兜底心跳又是 30 秒一次。
/// 三种节奏下"够久了"只有一个共同点:**连续失败了好几次**。
/// 这样既滤掉了抖动,又不会让用户等太久
const WATCH_FAIL_THRESHOLD: u32 = 10;

impl WatchState {
    /// 记一次"这轮没读到剪贴板",返回**该不该告诉用户监控其实没在工作**。
    ///
    /// 抽成方法是为了能测。这个判断本来藏在轮询循环里,而要触发它得让
    /// 真剪贴板连续读不到十次——那既造不出来,也不该在测试里造
    fn note_read_failure(&self) -> bool {
        let failures = self.read_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if failures < WATCH_FAIL_THRESHOLD {
            return false;
        }
        // 报完归零:故障持续下去还会再报,但不会每 700ms 刷一遍红条
        self.read_failures.store(0, Ordering::Relaxed);
        true
    }

    /// 读到了就归零。偶尔失败一下不算坏,连着失败才是
    fn note_read_ok(&self) {
        self.read_failures.store(0, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultInfo {
    pub path: String,
    pub watching: bool,
    /// 剪藏库建不出来时的原因,没有就是 `None`。
    ///
    /// **送代号不送句子**,和别处一个规矩:这句话得让前端翻语言。
    /// 不带这个字段的话,只读盘或网盘掉线时 `scan()` 会返回一个**空**列表,
    /// 界面上是个干干净净的空库、连红条都没有——用户的第一反应是
    /// "我的剪藏全没了",然后去翻硬盘、翻回收站、翻云同步
    pub problem: Option<WireError>,
    /// **问过用户要不要开剪贴板监控了没有。** 前端靠它决定空库那一屏上
    /// 要不要摆「要不要开启」那个按钮。
    ///
    /// 跟着 `VaultInfo` 走而不是单开一条命令:界面启动时本来就要拉一次这里
    /// 的信息,多一次 IPC 往返只为拿一个布尔值不划算
    #[serde(default)]
    pub watch_asked: bool,
}

/// 后台把图片下完之后的通知,让界面能告诉用户"图也存下来了"。
#[derive(Clone, Serialize)]
struct ImagesLocalized {
    filename: String,
    count: usize,
    /// 有几张没存下来。**这个数不能扔。** Quire 一直说自己"数据在你手上",
    /// 可图要是没下下来,`.md` 里还指着 CDN——用户点导出,拿到一份"本地图片
    /// 其实一张都没下下来"的 Markdown,换台设备全废,而 Quire 一声没吭。
    /// 失败的那些图仍保留远程地址,不会被改坏,所以必须说出来而不是装作齐了
    failed: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClipSavedNotice {
    pub id: String,
    pub filename: String,
}

fn current_vault(state: &State<AppState>) -> Result<Arc<Vault>, WireError> {
    // 锁中毒说明别的线程 panic 过,此时 vault 状态不可信,直接报错让用户重启
    state
        .vault
        .read()
        .map(|g| g.clone())
        .map_err(|_| WireError::new("vault.poisoned"))
}

fn info_of(state: &State<AppState>) -> VaultInfo {
    let watching = state.watch.enabled.load(Ordering::Relaxed);
    let Ok(vault) = current_vault(state) else {
        return VaultInfo {
            path: String::new(),
            watching,
            problem: Some(WireError::new("vault.poisoned")),
            ..Default::default()
        };
    };
    // **上次的目录不见了,优先报这件事。** 排在 `ensure_dirs` 前面,
    // 因为下面那个会把目录**建回来**——建回来之后就是个空库,
    // 用户看到"0 篇",只会以为剪藏全没了,而它们好好地躺在别处
    let gone = state
        .missing_vault
        .lock()
        .ok()
        .and_then(|g| g.clone());
    if let Some(gone) = gone {
        return VaultInfo {
            path: vault.root().display().to_string(),
            watching,
            problem: Some(WireError::new("vault.missing").with("path", gone.clone())),
            ..Default::default()
        };
    }
    // **每次问一次就真建一次。** 建目录是幂等的,代价可以忽略;
    // 不问的话,只读盘或者网盘掉线时 `scan()` 会返回一个空列表,
    // 界面上是个干干净净的空库、连红条都没有,用户只会以为剪藏全没了
    let problem = vault.ensure_dirs().err().map(|e| e.wire());
    VaultInfo {
        path: vault.root().display().to_string(),
        watching,
        problem,
        watch_asked: settings::Settings::load(&state.config_dir).watch_asked,
    }
}

pub(crate) fn hash_of(capture: &ClipboardCapture) -> u64 {
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
fn list_clips(state: State<AppState>) -> Result<ScanResult, WireError> {
    let vault = current_vault(&state)?;
    vault.scan().map_err(|e| e.wire())
}

#[tauri::command]
fn read_clip(filename: String, state: State<AppState>) -> Result<ClipContent, WireError> {
    let vault = current_vault(&state)?;
    vault.read_clip(&filename).map_err(|e| e.wire())
}

/// 全文检索。返回摘要 + 命中片段,前端直接拿去渲染列表。
///
/// **跳过的文件一起带回来**:读不出、没 frontmatter、没有 id 的 `.md`
/// 全都搜不到。悄悄跳掉的话,用户明明记得那篇里有这个词,搜出来是空的,
/// 而列表视图那边已经告诉过他有几篇读不出——他会以为是自己记错了
/// 搜到了,或者一条都没搜到时再放宽一次。
///
/// **放宽只在全空的时候做,不在结果少的时候做。** 结果少(比如 3 条)说明
/// 这个词是有效的,用户要的是更准不是更多;那时候再塞一批"差一个字"的进来,
/// 是拿他本来找得到的东西去换他本来也找不到的东西
///
/// 放宽那条路是**全库扫文件**,不走索引——索引分不出「差一个字」。
/// 只在零结果时才付这个钱,而零结果本来就是用户最需要帮忙的那一刻
fn search_with_fallback(
    dir: &std::path::Path,
    query: &str,
    limit: usize,
    scope: Option<search::Scope>,
    exact: &mut search::SearchOutcome,
) {
    if !exact.hits.is_empty() {
        return;
    }
    let loose = search::search_loose(dir, query, limit, scope);
    if !loose.is_empty() {
        exact.hits = loose;
    }
}

#[tauri::command]
fn search_clips(
    query: String,
    limit: Option<usize>,
    scope: Option<search::Scope>,
    state: State<AppState>,
) -> Result<SearchOutcome, WireError> {
    let vault = current_vault(&state)?;
    // 上限是防手滑的闸,不是业务规则。一次要一万条,界面也渲染不动。
    let limit = limit.unwrap_or(200).min(1000);
    let mut indexes = index_for(&state, &vault.clips_dir())?;
    let idx = indexes.get_mut(&vault.clips_dir()).expect("刚放进去的");
    // 索引建不起来、坏了、对不齐,都会退回逐文件扫描那条路。
    // 用户只会觉得慢了一点,不会察觉背后换了个实现
    let mut outcome = search::search_indexed(idx, &vault.clips_dir(), &query, limit, scope).0;
    search_with_fallback(&vault.clips_dir(), &query, limit, scope, &mut outcome);
    Ok(outcome)
}

/// 搜回收站。界面上回收站是独立视图,搜索框跟着它走。
/// 搜回收站。跳过的文件同样要报,理由和 `search_clips` 一样
#[tauri::command]
fn search_trash(
    query: String,
    limit: Option<usize>,
    scope: Option<search::Scope>,
    state: State<AppState>,
) -> Result<SearchOutcome, WireError> {
    let vault = current_vault(&state)?;
    let limit = limit.unwrap_or(200).min(1000);
    let dir = vault.trash_dir();
    let mut indexes = index_for(&state, &dir)?;
    let idx = indexes.get_mut(&dir).expect("刚放进去的");
    let mut outcome = search::search_indexed(idx, &dir, &query, limit, scope).0;
    search_with_fallback(&dir, &query, limit, scope, &mut outcome);
    Ok(outcome)
}

/// 改已读 / 归档 / 收藏标志。传 `None` 表示这一项不动。
#[tauri::command]
fn set_clip_flags(
    filename: String,
    read: Option<bool>,
    archived: Option<bool>,
    starred: Option<bool>,
    state: State<AppState>,
) -> Result<ClipSummary, WireError> {
    let vault = current_vault(&state)?;
    vault
        .set_flags(&filename, read, archived, starred)
        .map_err(|e| e.wire())
}

/// 导入一个文件夹里的 Markdown。**只读源目录**,源文件一个字节都不动。
///
/// 已经在库里的同一篇会被跳过并报出来——用户导进一个存过一堆旧文的文件夹,
/// 里面有一半是重复的,那是正常情况,不是失败。
#[tauri::command]
async fn import_markdown(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ImportReport, WireError> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    // recv 会一直阻塞到用户做出选择,而这是 async 命令:直接调会占死 worker
    // 线程,单线程 runtime 下就是彻底死锁
    let received = tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| WireError::internal(e.to_string()))?
        .map_err(|_| WireError::new("app.pickerFailed"))?;
    let Some(picked) = received else {
        return Err(WireError::new("app.cancelled"));
    };
    let path = picked.into_path().map_err(|e| WireError::internal(e.to_string()))?;

    // 一次可能导几百篇,读文件和写盘都不轻。挡在命令前面的话界面会假死
    let vault = current_vault(&state)?;
    let importer = vault.clone();
    let result = tauri::async_runtime::spawn_blocking(move || importer.import_markdown(&path))
        .await
        .map_err(|e| WireError::internal(e.to_string()))?
        .map_err(|e| e.wire())?;

    // 导入的剪藏也得把图下到本地,否则它们会永远指着原站。
    // **Quire 说自己剪藏时会存图,导入也是剪藏**,不存的话这批文章就成了一
    // 个洞:平时看不出问题,等原站关站那天,这批文章一起烂掉。
    // 逐篇 spawn,不排队:一次导 500 篇排着下的话,天都亮了
    for filename in &result.imported {
        spawn_image_localization(app.clone(), vault.clone(), filename.clone());
    }
    Ok(result)
}

/// 从别的稍后读工具的 CSV / JSON 导入。
///
/// 走文件对话框让用户自己挑文件:**替他在某个目录里猜哪个文件是要导的那个**,
/// 等于替他做决定,而这类文件他电脑里往往有好几个
#[tauri::command]
async fn import_data_file(app: AppHandle, state: State<'_, AppState>) -> Result<Option<import::DataImportReport>, WireError> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .add_filter("剪藏数据", &["csv", "json", "tsv"])
        .pick_file(move |picked| {
            let _ = tx.send(picked);
        });
    // recv 会阻塞到用户做出选择,而这是 async 命令:直接调会占死 worker 线程
    let received = tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| WireError::internal(e.to_string()))?
        .map_err(|_| WireError::new("app.pickerFailed"))?;
    let Some(picked) = received else {
        return Ok(None); // 用户点了取消,不是故障
    };
    let Some(path) = picked.into_path().ok() else {
        return Err(WireError::new("export.badPath"));
    };

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    // **读不出来就直接报错,不猜编码。** 猜错的后果是几百篇文章的标题
    // 全变乱码,而用户没有任何线索知道发生了什么
    let bytes = std::fs::read(&path)
        .map_err(|e| WireError::new("import.readFailed").with("detail", e.to_string()))?;
    let text = String::from_utf8(bytes).map_err(|_| WireError::new("import.badEncoding"))?;

    let vault = current_vault(&state)?;
    let importer = vault.clone();
    let report = tauri::async_runtime::spawn_blocking(move || importer.import_data(&text, &filename))
        .await
        .map_err(|e| WireError::internal(e.to_string()))?;

    // 报告里的错误**要变成命令的错误**:整批认不出来的时候,
    // 让它走 Result 的 Err 那条路,界面才有地方显示"不认这个格式"。
    // 成功但零篇的情况单独说一句,不然用户以为导成功了
    if let Some(err) = report.error.clone() {
        return Err(WireError::new("import.badFormat").with("detail", err));
    }

    // 导入的剪藏也得把图下到本地,否则它们会永远指着原站
    for f in &report.imported {
        spawn_image_localization(app.clone(), vault.clone(), f.clone());
    }
    Ok(Some(report))
}

/// 记读到哪儿了。**读的位置是用户的数据**,所以它和 read / archived 走同一条
/// 落盘路径:只重新序列化 frontmatter,正文一个字节都不动。
#[tauri::command]
fn set_clip_progress(
    filename: String,
    progress: f32,
    state: State<AppState>,
) -> Result<ClipSummary, WireError> {
    let vault = current_vault(&state)?;
    vault
        .set_progress(&filename, progress)
        .map_err(|e| e.wire())
}

/// 改一篇的标签。走 `set_flags` 同一条落盘路径,只重写 frontmatter。
#[tauri::command]
fn set_clip_tags(filename: String, tags: Vec<String>, state: State<AppState>) -> Result<ClipSummary, WireError> {
    let vault = current_vault(&state)?;
    vault.set_tags(&filename, &tags).map_err(|e| e.wire())
}

/// 改一篇的标题。空标题在入口就拒,理由见 `Vault::set_title`。
#[tauri::command]
fn set_clip_title(
    filename: String,
    title: String,
    stamp: Option<String>,
    state: State<AppState>,
) -> Result<ClipSummary, WireError> {
    let vault = current_vault(&state)?;
    vault.set_title(&filename, &title, stamp.as_deref()).map_err(|e| e.wire())
}

/// 写批注。允许清空——批注是可选的,标题不是,理由见 `Vault::set_note`。
#[tauri::command]
fn set_clip_note(
    filename: String,
    note: String,
    stamp: Option<String>,
    state: State<AppState>,
) -> Result<ClipSummary, WireError> {
    let vault = current_vault(&state)?;
    vault.set_note(&filename, &note, stamp.as_deref()).map_err(|e| e.wire())
}

/// 补全正文。剪藏那一刻抓不到网页、只存了用户复制的那一小段,
/// 几周后他想起来想看全文,靠这个补进去
///
/// **正文抽取仍然在前端做**,跟剪藏那一步同一条路:抓取在 Rust(Rust 有网络栈
/// 和 TLS),而抽取要 DOM、DOM 只在 webview 里。前端拿到 HTML,抽完再调这个
#[tauri::command]
fn set_clip_body(
    filename: String,
    markdown: String,
    stamp: Option<String>,
    state: State<AppState>,
) -> Result<ClipSummary, WireError> {
    let vault = current_vault(&state)?;
    vault.set_body(&filename, &markdown, stamp.as_deref()).map_err(|e| e.wire())
}

/// 这篇能不能补全文。**判断在前端做一遍、后端做一遍,不是冗余。**
///
/// 前端那遍是为了决定**要不要画那个按钮**;后端这遍是为了**不信任**——
/// 前端说什么都不算数,能不能补得看文件里到底有没有一个能抓的地址
#[tauri::command]
fn clip_source_url(filename: String, state: State<AppState>) -> Result<Option<String>, WireError> {
    let vault = current_vault(&state)?;
    let path = clip_file_path(&vault, &filename).map_err(|e| e.wire())?;
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let Some((block, _)) = frontmatter::split(&text) else {
        return Ok(None);
    };
    let url = frontmatter::Frontmatter::parse(block).url.trim().to_string();
    // 判一次"像不像地址",省得前端把一段乱七八糟的文字拿去发网络请求。
    // 用的是后端那条判据,不是前端另写一份——两份判据迟早对不上
    Ok(fetch::looks_fetchable(&url).then_some(url))
}

/// 库里有多少东西没做完:图还指着外站的、正文短得不像全文的、读不出的文件。
///
/// **界面只在真的有欠账时才摆那个入口。** 一个永远在那儿、点进去写着
/// 「一切都好」的按钮,是给人添乱的
#[tauri::command]
fn library_status(state: State<AppState>) -> Result<vault::LibraryStatus, WireError> {
    let vault = current_vault(&state)?;
    vault.library_status().map_err(|e| e.wire())
}

/// 标签栏的数据:每个标签 + 有几篇在用,按篇数倒序。
#[tauri::command]
fn list_tags(state: State<AppState>) -> Result<Vec<TagCount>, WireError> {
    let vault = current_vault(&state)?;
    vault.tag_index().map_err(|e| e.wire())
}

/// 移进回收站。**不真删**——剪藏工具里唯一能把用户东西弄没的操作,
/// 没必要一按就没。真要清空,用户自己去 `clips/.trash/` 里翻。
///
/// 一篇就是一批里只有一篇,走同一条路:批量最容易出的事就是"悄悄少做了一半",
/// 返回值必须能说出到底做了几篇、哪几篇没做成。
#[tauri::command]
fn trash_clips(filenames: Vec<String>, state: State<AppState>) -> Result<BatchReport, WireError> {
    let vault = current_vault(&state)?;
    vault.trash_batch(&filenames).map_err(|e| e.wire())
}

/// 一次改多篇的已读 / 归档。
#[tauri::command]
fn set_clip_flags_batch(
    filenames: Vec<String>,
    read: Option<bool>,
    archived: Option<bool>,
    starred: Option<bool>,
    state: State<AppState>,
) -> Result<BatchReport, WireError> {
    let vault = current_vault(&state)?;
    vault
        .set_flags_batch(&filenames, read, archived, starred)
        .map_err(|e| e.wire())
}

/// 一次给多篇打标签。逐条走 `set_tags` 的同一条路。
#[tauri::command]
fn set_clip_tags_batch(
    filenames: Vec<String>,
    tags: Vec<String>,
    state: State<AppState>,
) -> Result<BatchReport, WireError> {
    let vault = current_vault(&state)?;
    vault.set_tags_batch(&filenames, &tags).map_err(|e| e.wire())
}

/// 标签改名 / 合并。`from` 换成 `to`,`to` 已经有了就是合并(取并集)
///
/// **回收站里的也一起改**——只改剪藏库的话,哪天从回收站放回一篇,
/// 旧标签又回来了,用户会觉得"合了等于没合"
#[tauri::command]
fn rename_tag(
    from: String,
    to: String,
    state: State<AppState>,
) -> Result<TagRenameReport, WireError> {
    let vault = current_vault(&state)?;
    vault.rename_tag(&from, &to).map_err(|e| e.wire())
}

/// 从回收站放回原位。
#[tauri::command]
fn restore_clip(filename: String, state: State<AppState>) -> Result<ClipSummary, WireError> {
    let vault = current_vault(&state)?;
    vault.restore(&filename).map_err(|e| e.wire())
}

/// 一次放回多篇。批量删除的撤销走这条。
///
/// **有它是因为批量删除没有别的退路**:单篇删了弹一个撤销按钮,
/// 一次删 20 篇不弹的话,唯一的保险就变成"下次少勾两篇"。
/// 真正干活的是 `Vault::restore_batch`,那边有测试盯着一篇失败不拖累其余。
#[tauri::command]
fn restore_clips(
    filenames: Vec<String>,
    state: State<AppState>,
) -> Result<RestoredBatch, WireError> {
    let vault = current_vault(&state)?;
    vault.restore_batch(&filenames).map_err(|e| e.wire())
}

/// 回收站里剩下什么。**读不出元数据的也在列表里**,只是没有标题——
/// 那正是最该被看见、也最该能被清掉的一批。
#[tauri::command]
fn list_trash(state: State<AppState>) -> Result<TrashListing, WireError> {
    let vault = current_vault(&state)?;
    vault.scan_trash().map_err(|e| e.wire())
}

/// 预览回收站里某一篇的正文。看不到内容就没法判断该不该永久删。
#[tauri::command]
fn read_trash_clip(filename: String, state: State<AppState>) -> Result<ClipContent, WireError> {
    let vault = current_vault(&state)?;
    vault.read_trash_clip(&filename).map_err(|e| e.wire())
}

/// 彻底删除一篇。**没有撤销**,所以只作用于回收站里的文件。
#[tauri::command]
fn purge_clip(filename: String, state: State<AppState>) -> Result<(), WireError> {
    let vault = current_vault(&state)?;
    vault.purge(&filename).map_err(|e| e.wire())
}

/// 清空回收站。返回里带着清不掉的那些——"清空"没能清干净必须说出来。
///
/// **只是搬到隔离区。** 见 [`forget_all`] 才是真删
#[tauri::command]
fn empty_trash(state: State<AppState>) -> Result<BatchReport, WireError> {
    let vault = current_vault(&state)?;
    vault.empty_trash().map_err(|e| e.wire())
}

/// **真删一篇。** 全软件唯一不可撤销的操作,只认隔离区里的
#[tauri::command]
fn forget_clip(filename: String, state: State<AppState>) -> Result<(), WireError> {
    let vault = current_vault(&state)?;
    vault.forget(&filename).map_err(|e| e.wire())
}

/// **真删掉隔离区里的全部。** 这是唯一能把磁盘真正腾出来的地方
#[tauri::command]
fn forget_all(state: State<AppState>) -> Result<BatchReport, WireError> {
    let vault = current_vault(&state)?;
    vault.forget_all().map_err(|e| e.wire())
}

/// 把整个剪藏库拼成单个 Markdown,写到用户选的位置。
///
/// 走对话框让用户自己定存哪、叫什么名——导出是用户的动作,
/// 替他在某个目录里造个文件等于替他做决定。
#[tauri::command]
fn export_vault(app: AppHandle, state: State<AppState>) -> Result<Option<String>, WireError> {
    let vault = current_vault(&state)?;
    let markdown = vault.export_markdown().map_err(|e| e.wire())?;

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
        return Err(WireError::new("export.badPath"));
    };
    std::fs::write(&path, markdown)
        .map_err(|e| WireError::new("export.writeFailed").with("detail", e.to_string()))?;
    Ok(Some(path.display().to_string()))
}

/// 导出**选中的**那几篇。和 [`export_vault`] 走同一个保存对话框,
/// 但只拼名单里的那些——用户挑三篇发给别人,导出文件里塞两百篇的话,
/// 他得回去手工删干净,那还不如没有这个功能
#[tauri::command]
fn export_selected(
    app: AppHandle,
    state: State<AppState>,
    filenames: Vec<String>,
) -> Result<Option<String>, WireError> {
    let vault = current_vault(&state)?;
    let markdown = vault.export_selected(&filenames).map_err(|e| e.wire())?;

    let picked = app
        .dialog()
        .file()
        .set_file_name(default_selected_export_name(filenames.len()))
        .add_filter("Markdown", &["md"])
        .blocking_save_file();
    let Some(picked) = picked else {
        return Ok(None); // 用户点了取消,不是故障
    };
    let Some(path) = picked.into_path().ok() else {
        return Err(WireError::new("export.badPath"));
    };
    std::fs::write(&path, markdown)
        .map_err(|e| WireError::new("export.writeFailed").with("detail", e.to_string()))?;
    Ok(Some(path.display().to_string()))
}

/// 导出成一个**文件夹**,图片跟着走。
///
/// 走"选目录"而不是"选文件":导出来是一个目录。替用户先建好目录再往里写,
/// 等于替他决定存哪——而导出恰恰是最该他自己挑位置的动作
#[tauri::command]
async fn export_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    filenames: Option<Vec<String>>,
) -> Result<Option<vault::ExportFolder>, WireError> {
    let picker = app.clone();
    // `blocking_pick_folder` 会一直阻塞到用户做出选择。**必须放到 blocking 池里**,
    // 直接在 async 命令里调就是占死 worker 线程,单线程 runtime 下彻底死锁
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        picker
            .dialog()
            .file()
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
    })
    .await
    .map_err(|e| WireError::internal(e.to_string()))?;
    let Some(path) = picked else {
        return Ok(None); // 用户点了取消,不是故障
    };

    let vault = current_vault(&state)?;
    let report = vault
        .export_folder(&path, filenames.as_deref())
        .map_err(|e| e.wire())?;
    Ok(Some(report))
}

/// 选中了 1 篇就别写"1 篇",写得更像个人话一些
fn default_selected_export_name(n: usize) -> String {
    if n == 1 {
        format!("Quire-剪藏-{}.md", Local::now().format("%Y-%m-%d"))
    } else {
        format!("Quire-剪藏-{n}篇-{}.md", Local::now().format("%Y-%m-%d"))
    }
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
fn capture_clipboard() -> Result<ClipboardCapture, WireError> {
    clipboard::capture_clipboard()
}

#[tauri::command]
fn save_clip(
    app: AppHandle,
    input: ClipInput,
    force: Option<bool>,
    state: State<AppState>,
) -> Result<SaveOutcome, WireError> {
    let vault = current_vault(&state)?;
    // 判重放在真正落盘之前。同一篇文章存两遍,列表里就多一条一模一样的,
    // 用户得自己认出哪条是新的——那是在替软件擦屁股。
    let outcome = vault
        .save_checked(&input, force.unwrap_or(false))
        .map_err(|e| e.wire())?;
    if let SaveOutcome::Saved { id, filename } = &outcome {
        let _ = app.emit(
            "clip-saved",
            ClipSavedNotice {
                id: id.clone(),
                filename: filename.clone(),
            },
        );
        spawn_image_localization(app, vault, filename.clone());
    }
    Ok(outcome)
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
        if let Ok((saved, failed)) = result {
            // **一张都没存下来也要发。** 原来这里只在 `saved > 0` 时才发,
            // 于是"全部失败"这种最该说话的情况恰恰一声不吭。用户看到
            // 剪藏成功,以为图也存下来了,直到换设备才发现
            if saved > 0 || failed > 0 {
                let _ = app.emit(
                    "clip-saved-images",
                    ImagesLocalized {
                        filename,
                        count: saved,
                        failed,
                    },
                );
            }
        }
    });
}

/// 开关剪贴板监控。默认关闭:被动监听会连你复制的密码、验证码、快递单号
/// 一起捕获,当默认行为太吵,得由用户自己决定要不要。
#[tauri::command]
fn set_clipboard_watch(enabled: bool, state: State<AppState>) -> Result<VaultInfo, WireError> {
    // **关掉的时候要把监听停掉。** 只是把 enabled 置 false 的话,
    // 那个窗口还挂在系统的监听器列表上——用户不复制东西的时候它什么都不做,
    // 可它确实还占着一个窗口句柄,而且下一次复制时系统照样会通知它,
    // 线程照样醒着
    #[cfg(windows)]
    if !enabled {
        clipwatch::stop();
    }

    if enabled {
        // 开启的瞬间把剪贴板里现有的内容记成"已见"。否则用户刚打开开关,
        // 就会被自己几分钟前复制的东西弹一次提示,平白觉得这东西在窥探。
        if let Ok(capture) = clipboard::capture_clipboard() {
            state
                .watch
                .last_hash
                .store(hash_of(&capture), Ordering::Relaxed);
        }
    }
    state.watch.enabled.store(enabled, Ordering::Relaxed);
    // 开关也记下来。用户明确开过一次,每次启动都弹回"关"是在替他做决定
    let mut cfg = settings::Settings::load(&state.config_dir);
    cfg.watch_clipboard = enabled;
    // 用户在工具栏上自己拨了开关,那就**不用再问要不要开了**——
    // 他已经用行动回答过,再弹一次是拿他的决定当没发生过
    cfg.watch_asked = true;
    if let Err(reason) = cfg.save(&state.config_dir) {
        eprintln!("[quire] 配置存不进去:{reason}");
    }
    Ok(info_of(&state))
}

/// 把一个网址的文章抓下来。**抓到的只是 HTML,正文抽取在前端做**——
/// 抽取要 DOM,而 DOM 只存在于 webview 里
///
/// 返回 `Err` 不是故障,是「这个页面抓不到」:需要登录的、纯 JS 渲染的、
/// 被拦的,都属于这一类。界面上该说的是「抓不到全文,已存你复制的内容」,
/// 而不是弹一条红条
#[tauri::command]
async fn fetch_article(url: String) -> Result<FetchedPage, WireError> {
    if !fetch::looks_fetchable(&url) {
        return Err(WireError::new("fetch.notHttp").with("url", url));
    }
    let target = url.clone();
    // 抓页面要走网络,几十秒都有可能。**挡在命令前面会界面假死**
    let got = tauri::async_runtime::spawn_blocking(move || fetch::fetch_page(&target))
        .await
        .map_err(|e| WireError::internal(e.to_string()))?;
    match got {
        Ok(f) => Ok(FetchedPage {
            html: f.html,
            url: f.final_url,
        }),
        Err(e) => Err(fetch_error_wire(e)),
    }
}

/// 抓取失败的原因 → 代号。**每一类给不同的说法**:
/// 「连不上」重试可能成,「这页不是 HTML」重试一万次也不成
fn fetch_error_wire(e: fetch::FetchError) -> WireError {
    let code = match &e {
        fetch::FetchError::NotHttp(_) => "fetch.notHttp",
        fetch::FetchError::NotHtml(_) => "fetch.notHtml",
        fetch::FetchError::Timeout => "fetch.timeout",
        fetch::FetchError::TooLarge(_) => "fetch.tooLarge",
        fetch::FetchError::Network(_) => "fetch.failed",
    };
    let detail = match &e {
        fetch::FetchError::NotHttp(u) => u.clone(),
        fetch::FetchError::NotHtml(t) => format!("返回的是 {t},不是网页"),
        fetch::FetchError::TooLarge(n) => {
            format!("{n} 字节,超过 {}", fetch::MAX_HTML_BYTES)
        }
        other => format!("{other:?}"),
    };
    WireError::new(code).with("detail", detail)
}

/// 抓回来的页面。字段名与 Rust 侧 `Fetched` 一致
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedPage {
    /// 页面的 HTML。**整份送进 webview**,正文抽取在前端做
    pub html: String,
    /// 最终地址。页面可能 301 跳到别处,而正文里的相对链接要按它解析
    pub url: String,
}

/// 弹出目录选择器,选完立刻切换并广播。取消选择返回 None,保持原目录不变。
#[tauri::command]
async fn pick_vault(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<VaultInfo>, WireError> {
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
        .map_err(|e| WireError::internal(e.to_string()))?
        .map_err(|_| WireError::new("app.pickerFailed"))?;

    let Some(picked) = received else {
        return Ok(None); // 用户取消,保持原目录
    };
    let path = picked.into_path().map_err(|e| WireError::internal(e.to_string()))?;

    let vault = Vault::new(&path);
    vault.ensure_dirs().map_err(|e| e.wire())?;
    // **记下来。** 不记的话用户每次重启都要重指一遍,而剪藏好好地
    // 躺在那儿——对一个 local-first 工具来说这是最伤的一处
    let mut cfg = settings::Settings::load(&state.config_dir);
    cfg.vault_path = Some(path);
    // 存不进去也说一句,但**不打断他选目录**——目录已经指好了,
    // 存不住只是下次要重选一次,为此把这次的成果丢掉才叫蠢
    if let Err(reason) = cfg.save(&state.config_dir) {
        eprintln!("[quire] 配置存不进去:{reason}");
    }
    {
        let mut guard = state.vault.write().map_err(|_| WireError::new("vault.poisoned"))?;
        *guard = Arc::new(vault);
    }
    // 重新指过目录了,"上次的目录不见了"这件事就不再成立。
    // 忘了清的话,用户明明已经指对了还一直看着那条红条
    if let Ok(mut missing) = state.missing_vault.lock() {
        *missing = None;
    }
    let info = info_of(&state);
    let _ = app.emit("vault-changed", &info);
    Ok(Some(info))
}

/// 在系统文件管理器里打开剪藏目录——这是"数据在你手上"最直观的一次兑现,
/// 用户随时能看见、随时能拷走。
#[tauri::command]
fn open_vault_folder(state: State<AppState>) -> Result<(), WireError> {
    let vault = current_vault(&state)?;
    vault.ensure_dirs().map_err(|e| e.wire())?;

    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");

    cmd.arg(vault.root())
        .spawn()
        .map(|_| ())
        .map_err(|e| WireError::new("export.openFolderFailed").with("detail", e.to_string()))
}

/// 记下"上次打开的是哪一篇",下次启动放回原处
///
/// **只记真的读了多久的那几篇。** 翻列表时经过一篇不算——那种"读过"
/// 记下来,用户下次回来发现自己被放在一篇随手划过去的文章上,只会觉得
/// 软件在乱跳
///
/// **存不进去不报错。** 记住上次读到哪是锦上添花,为一个配置文件失败
/// 而弹红条,是在用一条红杠换一个可有可无的便利
#[tauri::command]
fn remember_last_read(filename: String, state: State<AppState>) -> Result<(), WireError> {
    if !slug::is_safe_filename(&filename) {
        return Err(WireError::new("vault.unsafeFilename").with("detail", filename));
    }
    let mut cfg = settings::Settings::load(&state.config_dir);
    // 值没变就别写。配置每次启动都读,值一样还写只会白白惊动磁盘
    if cfg.last_read.as_deref() == Some(filename.as_str()) {
        return Ok(());
    }
    cfg.last_read = Some(filename);
    if let Err(reason) = cfg.save(&state.config_dir) {
        eprintln!("[quire] 上次读到哪记不进去:{reason}");
    }
    Ok(())
}

/// 上次打开的是哪一篇。启动时前端拿它决定落在详情页还是列表
///
/// 那一篇已经不在了(被删了、换了剪藏库、导入了另一个目录)时**返回空**,
/// 不是报错——用户要的是"回到上次那篇",那篇没了就回列表,和它还在的
/// 时候一样自然
#[tauri::command]
fn last_read(state: State<AppState>) -> Option<String> {
    let cfg = settings::Settings::load(&state.config_dir);
    let name = cfg.last_read?;
    if !slug::is_safe_filename(&name) {
        return None;
    }
    let vault = current_vault(&state).ok()?;
    vault.clips_dir().join(&name).is_file().then_some(name)
}

/// 记一次搜过的词。**记下来是因为用户搜不到的时候想不起上次搜了什么**——
/// 他记得有那篇文章,想不起它叫什么词,这时候"你刚才搜过这些"比任何
/// 提示都管用
#[tauri::command]
fn remember_search(query: String, state: State<AppState>) -> Result<(), WireError> {
    let mut cfg = settings::Settings::load(&state.config_dir);
    // 先问一句"有没有变化",再决定写不写。配置是每次启动都读的,
    // 值一样还去写只会白白惊动磁盘
    let before = cfg.recent_searches.clone();
    settings::push_recent_search(&mut cfg.recent_searches, &query);
    if cfg.recent_searches == before {
        return Ok(());
    }
    if let Err(reason) = cfg.save(&state.config_dir) {
        // 记不住最近搜过什么不算故障,只是下次少一条建议。
        // 弹红条是拿一件小事去吓一个正要找东西的人
        eprintln!("[quire] 最近搜索记不进去:{reason}");
    }
    Ok(())
}

/// 最近搜过的词。**最多 10 条,新的在前**
#[tauri::command]
fn recent_searches(state: State<AppState>) -> Vec<String> {
    settings::Settings::load(&state.config_dir).recent_searches
}

/// 用系统默认程序打开某一篇的 `.md`,或者在文件管理器里定位它
///
/// README 上写着"数据是你的"。兑现这句话不该只能"打开整个剪藏目录"——
/// 用户想拿 Obsidian 看某篇,或者想在文件管理器里确认某个附件到底在不在,
/// 现在都得自己去文件夹里按 `2026-09-30-0munof…-example-com.md` 找
///
/// **`reveal` 是两种行为的唯一区别**:定位用文件管理器选中的高亮,
/// 打开用默认程序。macOS 的 `open -R` 就是定位;Linux 的文件管理器没有
/// "定位"这个动作,退化成打开所在目录
#[tauri::command]
fn open_clip_file(
    filename: String,
    reveal: Option<bool>,
    state: State<AppState>,
) -> Result<(), WireError> {
    let vault = current_vault(&state)?;
    let path = clip_file_path(&vault, &filename).map_err(|e| e.wire())?;
    reveal_or_open(&path, reveal.unwrap_or(false))
}

/// 把界面传来的文件名变成剪藏库里的绝对路径
///
/// **单独抽出来是为了能测。** 命令函数要 `State<AppState>`,单测里建不出
/// 那个状态;而这个函数是**整条链上唯一做安全判断的地方**,不测它就等于
/// 把最要紧的一环留在"看代码觉得没问题"的档次上
fn clip_file_path(vault: &Vault, filename: &str) -> Result<PathBuf, VaultError> {
    // **文件名必须过安全闸。** 这个接口吃的是外部传进来的字符串,
    // 少了 `is_safe_filename` 就等于开了个任意路径穿越:一个
    // `../../启动文件夹.md` 这样的文件名能把 Quire 变成任意文件启动器
    if !slug::is_safe_filename(filename) {
        return Err(VaultError::UnsafeFilename(filename.to_string()));
    }
    let path = vault.clips_dir().join(filename);
    if !path.is_file() {
        return Err(VaultError::NotFound(filename.to_string()));
    }
    Ok(path)
}

/// 打开一个路径:传目录就是打开目录,传文件就是用默认程序打开。
/// 三个平台同一条命令,只是参数不同
fn reveal_or_open(path: &Path, reveal: bool) -> Result<(), WireError> {
    #[cfg(target_os = "windows")]
    let mut cmd = if reveal {
        let mut c = std::process::Command::new("explorer");
        // explorer 有个老毛病:把参数当选项解析,`/select,` 后面**不能有空格**
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        // 打开文件不能直接用 explorer:它会用资源管理器打开 .md,
        // 而不是交给默认程序。`cmd /C start` 才是"用默认程序打开"
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        if reveal {
            c.arg("-R");
        }
        c.arg(path);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        // Linux 的文件管理器没有"定位"这个动作,退化成打开所在目录
        c.arg(if reveal { path.parent().unwrap_or(path) } else { path });
        c
    };

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| WireError::new("clip.openFailed").with("detail", e.to_string()))
}

/// 启动时决定用哪个剪藏库目录,以及要不要报"上次的目录不在了"。
///
/// 拆出来是为了能测:`setup` 闭包里那套逻辑在单测里调不到, 而"目录
/// 没了怎么办"恰恰是最该测的一处——它决定用户会不会以为剪藏全没了
fn resolve_vault_dir(cfg: &settings::Settings) -> (PathBuf, Option<String>) {
    match cfg.vault_dir() {
        Some(saved) if saved.is_dir() => (saved.to_path_buf(), None),
        // 上次指的地方没了:记住它,但**别在这儿建目录**。
        // 建出来就是个空库,用户看到"0 篇",会以为剪藏全没了,
        // 而它们好好地躺在别处(硬盘掉了、网盘没挂、同步盘换了路径)
        Some(saved) => (saved.to_path_buf(), Some(saved.display().to_string())),
        // 没指过,或者配置读不出来:用默认目录。那是第一次运行的常态
        None => (default_vault_dir(), None),
    }
}

fn default_vault_dir() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Quire")
}

/// 后台轮询剪贴板。默认关闭,只把内容**通知**给前端,不自动保存——
/// 自动存等于替用户做决定,而且监控开着的时候什么都会往里灌。
/// 盯着剪藏目录,有外部改动就通知前端重扫。
///
/// **只报"该重扫了",不报改了哪些文件。** 重扫的时候自然会看到具体变化,
/// 而攒一份文件列表要在锁里攒,还得处理一半是临时文件的垃圾
fn start_folder_watch(
    app: AppHandle,
    dir: PathBuf,
    guard: watcher::WriteGuard,
) -> Option<()> {
    use notify::{RecursiveMode, Watcher};

    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut w = notify::recommended_watcher(move |res| {
        // **回调里不能做重活。** 它跑在系统的通知线程上,
        // 在这儿读文件会把整个事件队列堵住
        let _ = tx.send(res);
    })
    .ok()?;
    for target in watcher::watch_targets(&dir) {
        if w.watch(&target, RecursiveMode::NonRecursive).is_err() {
            // 目录还没建出来的话就算了。**这不是故障**——剪藏库还不存在
            // 是正常状态(用户刚装完还没剪过),而这个 app 从来没有
            // 「目录消失」这一说
            continue;
        }
    }
    std::thread::spawn(move || {
        let mut debouncer = watcher::Debouncer::default();
        loop {
            // **睡到有话说的时候。** 挂着事件就等那个静默期,空着就等一个
            // 很久的时间——**没挂着的时候没有任何东西需要判断**,
            // 而一秒一醒是什么都不干的空转,监控是要整天开着的
            let nap = debouncer.waiting().unwrap_or(watcher::IDLE_WAIT);
            match rx.recv_timeout(nap) {
                Ok(Ok(event)) => {
                    // Quire 自己写的盘不算数。**这一条是整个监控能不能用的关键**:
                    // 不排除的话,每一次剪藏都紧跟着一次全量重扫
                    if watcher::worth_noticing(&guard, &event.paths) {
                        debouncer.push();
                    }
                    // 值得收的才收,不值得收的就**什么都不做**。
                    // 别顺手 `reset()`——用户刚改完文件、Quire 同时剪藏了一次,
                    // 那次改动会被抹掉,而这一轮再没有新事件的话,
                    // 它要等到下次手动重扫才出现
                }
                // 错误事件本身不代表磁盘变了。**报错就继续等**,
                // 监听器还活着;真断了的话 recv 会一直返回 Disconnected
                Ok(Err(_)) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                // 通道断了说明发送端没了,再等下去只是空转
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if debouncer.ready() {
                let _ = app.emit("vault-changed", ());
            }
        }
    });
    Some(())
}

/// 剪贴板内容变了之后的处理。**监听那条路和轮询那条路共用这一个函数**——
/// 两份实现意味着两种行为,改一处忘了另一处,用户看到的现象是
/// 「有时候弹有时候不弹」
pub(crate) fn on_clipboard_changed(app: &AppHandle, state: &Arc<WatchState>) {
    if !state.enabled.load(Ordering::Relaxed) {
        return;
    }
    let Ok(capture) = clipboard::capture_clipboard() else {
        if state.note_read_failure() {
            let _ = app.emit("clipboard-watch-broken", ());
        }
        return;
    };
    state.note_read_ok();
    if capture.is_empty() {
        return;
    }
    // **哈希去重照旧。** Windows 在程序自己改剪贴板时也会发通知,
    // 不去重的话用户复制一次会弹两条
    let hash = hash_of(&capture);
    if state.last_hash.swap(hash, Ordering::Relaxed) == hash {
        return;
    }
    let _ = app.emit("clipboard-changed", capture);
}

/// 监听窗口的过程要拿到 `AppHandle`。**存成全局**是因为窗口过程是
/// 一个 `extern "system"` 函数,拿不到任何闭包里的东西
static CLIP_APP: Mutex<Option<AppHandle>> = Mutex::new(None);

pub(crate) fn clip_app() -> Option<AppHandle> {
    CLIP_APP.lock().ok().and_then(|a| a.clone())
}

fn start_clipboard_watch(app: AppHandle, state: Arc<WatchState>) {
    // 监听和轮询**只能有一条在跑**,但都要留着对方的兜底
    #[cfg(windows)]
    {
        if let Ok(mut slot) = CLIP_APP.lock() {
            *slot = Some(app.clone());
        }
        if clipwatch::start(state.clone()).is_some() {
            // 监听成了。**仍然起一个慢速心跳**,不为零的间隔:
            // 万一某条通知没派发到,几十秒之内还有下一次机会。
            // 用户看到的差别只是「慢了几秒」和「彻底不弹」,中间没有过渡
            std::thread::spawn(move || loop {
                std::thread::sleep(clipwatch::FALLBACK_POLL);
                if clipwatch::alive() {
                    on_clipboard_changed(&app, &state);
                }
            });
            return;
        }
        // 监听没成(会话 0、远程桌面断开、太老的 Windows),
        // **退回去轮询**。宁可慢也不能不响应——监控失效意味着用户
        // 复制的东西根本不弹提示,而那是这个软件唯一的使用方式
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(WATCH_INTERVAL_MS));
        on_clipboard_changed(&app, &state);
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| default_vault_dir().join(".config"));
            let cfg = settings::Settings::load(&config_dir);

            let (dir, missing) = resolve_vault_dir(&cfg);

            let vault = Vault::new(dir);
            // 目录建不出来也要把应用起起来——用户可能选了只读盘或网络盘,
            // 让他在界面上看到真实报错,好过窗口都开不出来。
            // **这句报错由 `info_of` 兑现**:它每次被问都会真建一次目录,
            // 建不出来就把 WireError 带出去,前端弹红条。这里只是提前试一次
            let _ = vault.ensure_dirs();

            let watch = Arc::new(WatchState::default());
            // 上次是开着的就接着开。用户明确开过一次,启动就弹回"关"
            // 是在替他做决定
            watch.enabled.store(cfg.watch_clipboard, Ordering::Relaxed);

            // 到期的隔离文件顺手清一遍。**放这儿,不放进扫库那条路**:
            // 扫库是用户看得见的动作(他在等列表出来),往里塞一段
            // 文件删除不是他点的。顺带一提,清不掉的不打断启动——
            // 文件被别的程序占着是常事,红条弹出来只会让人以为剪藏出事了
            // `Vault` 不是 Clone,也不需要 Clone:清扫只要一个根路径,
            // 拿它单独造一个就够,不用把整个状态锁传进后台线程。
            // **不抢 vault 的锁**,所以它跟用户同时在进行的操作互不干扰
            let sweep_root = vault.root().to_path_buf();
            std::thread::spawn(move || {
                if let Err(reason) = Vault::new(&sweep_root).sweep_deleted() {
                    eprintln!("[quire] 清隔离区失败:{reason}");
                }
            });

            start_clipboard_watch(handle, watch.clone());
            // **必须拿 vault 自己那一份。** 另建一个的话,写盘那边记的是
            // A 对象、监控这边问的是 B 对象,两者永远对不上——
            // 排除失效,每一次剪藏都紧跟着一次全量重扫
            let folder_watch = start_folder_watch(
                app.handle().clone(),
                vault.clips_dir(),
                vault.write_guard(),
            );
            let _ = folder_watch; // 监听器的生命周期挂在后台线程上,这里只是记一笔

            app.manage(AppState {
                vault: vault::shared(vault),
                watch,
                config_dir,
                missing_vault: Mutex::new(missing),
                search_index: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            vault_info,
            list_clips,
            read_clip,
            search_clips,
            set_clip_flags,
            import_markdown,
            set_clip_progress,
            set_clip_tags,
            set_clip_title,
            set_clip_note,
            list_tags,
            set_clip_tags_batch,
            rename_tag,
            remember_last_read,
            set_clip_body,
            clip_source_url,
            library_status,
            remember_search,
            recent_searches,
            last_read,
            trash_clips,
            set_clip_flags_batch,
            list_trash,
            import_data_file,
            export_selected,
            export_folder,
            read_trash_clip,
            search_trash,
            purge_clip,
            forget_clip,
            forget_all,
            empty_trash,
            restore_clip,
            restore_clips,
            export_vault,
            capture_clipboard,
            save_clip,
            set_clipboard_watch,
            fetch_article,
            pick_vault,
            open_vault_folder,
            open_clip_file
        ])
        .run(tauri::generate_context!())
        .expect("Quire 启动失败");
}

#[cfg(test)]
// 测试名用中文描述行为本身,比 snake_case 的机翻名字好读
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 读本文件源码。**下面那几条要靠它,因为要验的东西在 Tauri 的
    /// `setup` 闭包和命令函数里,单测根本调不到。**
    ///
    /// 这不是耍花招:上面那些测的是 `Settings::save/load` 和
    /// `resolve_vault_dir` 各自对不对,而真正的失败模式是**两个都对、
    /// 但没接上**——存了没人读,或者读了没人存。那种 bug 跑一百遍单测
    /// 也是绿的,只有对着源码看才看得见
    const SOURCE: &str = include_str!("lib.rs");
    /// 前端源码。同一个数在两边各有一份,得能对着看
    const WEB: &str = include_str!("../../src/main.ts");

    /// 正式代码那一段,**测试模块自己不算**。
    ///
    /// `SOURCE` 是本文件全文,连测试模块一起。而"找某段字符串"的断言,
    /// 那个字符串要是原样写在断言里,它就必然在自己身上命中——
    /// **永远为真,永远绿,其实什么都没验**。这类假绿比红更坏:
    /// 它让人以为接线验过了,实际上一行都没生效过
    fn prod_source() -> &'static str {
        let cut = SOURCE
            .find("#[cfg(test)]\n// 测试名用中文描述行为本身")
            .expect("源码里找不到测试模块的起点");
        &SOURCE[..cut]
    }

    /// `prod_source()` 自己得先是对的。**上面那条 helper 也是本文件里的代码**,
    /// 它切错了不会有任何测试告诉人
    #[test]
    fn prod_source得切在测试模块之前() {
        let p = prod_source();
        assert!(!p.contains("mod tests"), "切点不对:测试模块被算进正式代码了");
        assert!(
            p.contains("fn set_clipboard_watch"),
            "切点不对:正式代码被切掉了一截"
        );
        assert!(
            p.contains("pub fn run()"),
            "切点不对:`run()` 都在正式代码里"
        );
    }

    /// 从源码里抠出某个函数/块的正文
    fn body_of(src: &str, needle: &str) -> String {
        let start = src.find(needle).unwrap_or_else(|| panic!("源码里找不到 {needle}"));
        let rest = &src[start..];
        let Some(open) = rest.find('{') else {
            panic!("{needle} 后面没有 {{");
        };
        let mut depth = 0usize;
        for (i, c) in rest[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return rest[..open + i + 1].to_string();
                    }
                }
                _ => {}
            }
        }
        panic!("{needle} 的花括号没闭合");
    }

    /// 选了目录要写回配置。**不写的话用户每次重启都要重指一遍**
    #[test]
    fn 选了目录要存回配置() {
        let body = body_of(prod_source(), "async fn pick_vault");
        assert!(body.contains("cfg.vault_path = Some(path)"), "选完没记路径");
        assert!(body.contains("cfg.save("), "记了但没落盘");
    }

    /// 启动要读回来。**存了没人读跟没存是一回事**
    #[test]
    fn 启动要读回配置() {
        let body = body_of(prod_source(), ".setup(|app|");
        assert!(body.contains("settings::Settings::load(&config_dir)"), "启动没读配置");
        assert!(body.contains("resolve_vault_dir(&cfg)"), "读了配置却没拿它定目录");
    }

    /// 监控开关也要记也要读,**两个方向都要**
    #[test]
    fn 监控开关的存取两头都在() {
        let setter = body_of(prod_source(), "fn set_clipboard_watch");
        assert!(setter.contains("cfg.watch_clipboard = enabled"), "改开关没记");
        let setup = body_of(prod_source(), ".setup(|app|");
        assert!(
            setup.contains("watch.enabled.store(cfg.watch_clipboard"),
            "启动没把开关恢复回去"
        );
    }

    /// **用户自己拨了开关就算"回答过要不要开"。**
    ///
    /// 少这一句的话,用户明明在工具栏上开了监控,启动后空库那一屏上
    /// 还是摆着「要不要开启」——他刚刚才做过那个决定,再问一次是在
    /// 拿他的决定当没发生过
    #[test]
    fn 自己拨开关就记成已经问过() {
        let setter = body_of(prod_source(), "fn set_clipboard_watch");
        assert!(
            setter.contains("cfg.watch_asked = true"),
            "自己拨开关没记成已回答"
        );
    }

    /// **`VaultInfo` 要把这个值下发出去。** 前端靠它决定空库那一屏上
    /// 摆不摆那个按钮,不下发的话前端拿到的是 undefined,
    /// 而 `!undefined` 是 true —— 每次空库都会弹一次同样的问题
    #[test]
    fn vault_info带着这个值() {
        let f = body_of(prod_source(), "fn info_of");
        assert!(
            f.contains("watch_asked: settings::Settings::load"),
            "VaultInfo 没下发 watch_asked"
        );
    }

    /// **会写盘的方法一个都不许漏 `note_write`。**
    ///
    /// 漏一个的后果不是"某个功能坏了",是"用了那个功能之后列表莫名重扫
    /// 一下"——而它只在那一个操作下出现,极难复现,更难定位。
    ///
    /// **用源码扫描而不是运行时验证**,是因为运行时验证只能覆盖
    /// 你想到的那几个操作。列全了就是列全了
    #[test]
    fn 每个写盘的方法都记得自己写过() {
        const WRITERS: &[&str] = &[
            "pub fn save", "pub fn save_checked", "pub fn set_flags", "pub fn set_title",
            "pub fn set_progress", "pub fn set_tags", "pub fn rename_tag", "pub fn set_note",
            "pub fn set_tags_batch", "pub fn trash(&self", "pub fn restore", "pub fn purge",
            "pub fn forget", "pub fn forget_all", "pub fn set_flags_batch",
            "pub fn trash_batch", "pub fn restore_batch", "pub fn store_imported",
            "pub fn set_body",
        ];
        let src = include_str!("vault.rs");
        let missing: Vec<&str> = WRITERS
            .iter()
            .filter(|w| !body_of(src, w).contains("self.note_write()"))
            .copied()
            .collect();
        assert!(
            missing.is_empty(),
            "这些方法写了盘但没记,用了就会莫名触发全量重扫:{missing:?}"
        );
    }

    /// **写盘那边和监控那边必须是同一个对象。** 复制一份的话两边各记各的,
    /// 排除完全失效——而那个失效表现为"剪藏之后列表跳一下",
    /// 没有报错、没有日志、什么线索都不留
    #[test]
    fn 写盘和监控共用同一份记录() {
        let prod = prod_source();
        assert!(
            prod.contains("vault.write_guard()"),
            "监控线程没有拿 vault 自己那一份"
        );
        // 不能另建一个。
        // **搜的是 `prod_source()` 而不是全文**——这个多行串原样写在本测试里,
        // 拿全文去搜必然命中自己,断言恒为真,看着绿其实一行都没验过
        assert!(
            !prod.contains("start_folder_watch(
                app.handle().clone(),
                vault.clips_dir(),
                watcher::WriteGuard::new()"),
            "监控线程另建了一个对象,写盘那边记的和它问的不是一回事"
        );
    }

    /// **自己写盘那条事件只能丢它自己,不能把攒着的清掉。**
    ///
    /// 这不是洁癖:用户改完文件存盘、同一瞬间在 Quire 里剪藏了另一篇,
    /// 那一次改动就被抹掉了。而这一轮不会再有新事件,
    /// 它要等到用户下次手动重扫才出现——"我明明改了它怎么没动"
    #[test]
    fn 自己写的那条不许清掉攒着的() {
        let body = body_of(prod_source(), "fn start_folder_watch");
        assert!(
            !body.contains("debouncer.reset()"),
            "收到自己写盘的事件时把攒着的一起清了,用户同时段的手工改动就没了"
        );
    }

    /// **空着的时候别一秒一醒。** 监控是要整天开着的,而没挂着事件时
    /// 根本没有任何东西需要判断——定频轮询是纯浪费。
    /// 顺带把断开的处理也盯住:通道断了还在 `try_recv` 就是空转到天荒地老
    #[test]
    fn 空等时不空转() {
        let body = body_of(prod_source(), "fn start_folder_watch");
        assert!(
            body.contains("watcher::IDLE_WAIT"),
            "空等的间隔得用 watcher::IDLE_WAIT 那个长值,别就地写个秒数——写小了没人拦得住"
        );
        assert!(
            !body.contains("std::thread::sleep"),
            "回到 sleep+轮询了:事件不来也一直醒着,CPU 白烧"
        );
        assert!(
            body.contains("recv_timeout"),
            "得让通道本身来唤醒:事件一来立刻醒,没事件就睡着"
        );
        assert!(
            body.contains("RecvTimeoutError::Disconnected"),
            "监听器断了之后要退出循环,不能空转到进程结束"
        );
    }

    /// 重新指过目录了,"上次的目录不见了"这条红条就得撤掉。
    /// 忘了撤的话,用户明明已经指对了还一直看着那条红条,
    /// 他会以为 Quire 认定他的剪藏没了
    #[test]
    fn 重新指过目录要撤掉那条红条() {
        let body = body_of(prod_source(), "async fn pick_vault");
        assert!(
            body.contains("*missing = None"),
            "换了目录还一直报'上次的目录不见了'"
        );
    }

    /// 记了就得能用:配置目录得真的传给了 `AppState`
    #[test]
    fn 配置目录得进状态里() {
        let setup = body_of(prod_source(), ".setup(|app|");
        assert!(
            setup.contains("config_dir,"),
            "config_dir 没进 AppState,后面存配置会没地方存"
        );
    }

    /// 搜索命令得真的走索引那条路。
    ///
    /// `search::search_indexed` 有三十几条测试全绿,但那**只证明它本身是对的**。
    /// 它要是没被命令调上,生产里走的还是逐文件扫描,整个 FTS5 就是一段
    /// 没人执行的死代码——而且所有测试都绿着,没有一条会红
    #[test]
    fn 搜索命令得走索引那条路() {
        for cmd in ["fn search_clips", "fn search_trash"] {
            let body = body_of(prod_source(), cmd);
            assert!(
                body.contains("search::search_indexed("),
                "{cmd} 没走索引,还在逐文件扫描"
            );
        }
    }

    /// **零结果时那条容错的路得接上。**
    ///
    /// `search_loose` 有一批测试全绿,但那**只证明它本身是对的**。
    /// 它要是没被命令调上,搜不到还是搜不到——用户看到的就是一个死路,
    /// 而所有测试都绿着。搜不到是整个产品最需要帮忙的那一刻,
    /// 那一刻不帮忙,这条功能等于没做
    #[test]
    fn 零结果时得走容错那条路() {
        for cmd in ["fn search_clips", "fn search_trash"] {
            let body = body_of(prod_source(), cmd);
            assert!(
                body.contains("search_with_fallback("),
                "{cmd} 没接容错那条路,搜不到还是死路"
            );
        }
    }

    /// **容错只在全空的时候放宽。** 放宽的位置写错——比如放在
    /// "结果少于 N 条就放宽"——用户搜「苹果」会得到一堆「苹朵」「苹果派」,
    /// 把本来找得到的东西挤出去。**拿他找得到的换他找不到的,更糟。**
    #[test]
    fn 容错只在全空的时候才放宽() {
        let body = body_of(prod_source(), "fn search_with_fallback");
        assert!(
            body.contains("if !exact.hits.is_empty()"),
            "没判空就往下走:有结果时也塞容错结果,好结果被挤出去了"
        );
        assert!(
            body.contains("return;"),
            "有结果时没有提前返回"
        );
    }

    /// **「要不要抓全文」的那个字数,两边必须一样。**
    ///
    /// 前端判断值不值得去抓,后端 `library_status` 判断这一篇是不是当初没抓到——
    /// 两处各写一份的话,改漏的后果是「有的篇能补有的不能补」,
    /// 用户完全看不出规律,只会觉得这个功能时灵时不灵
    #[test]
    fn 抓全文的字数阈值两边一致() {
        let web = WEB
            .split("const FULL_TEXT_MIN_CLIP_CHARS")
            .nth(1)
            .and_then(|s| s.split("=").nth(1))
            .and_then(|s| s.split(";").next())
            .map(|s| s.trim())
            .unwrap_or("");
        assert_eq!(
            web,
            crate::fetch::MIN_CLIP_CHARS_FOR_FETCH.to_string(),
            "前端的 FULL_TEXT_MIN_CLIP_CHARS 和后端的 MIN_CLIP_CHARS_FOR_FETCH 对不上:"
        );
    }

    /// **这轮新加的三个命令都得在注册表里。**
    ///
    /// `set_clip_body` / `clip_source_url` / `library_status` 三个函数写对了、
    /// 也有各自的单测,但**没注册就等于没有**——前端调过去是「命令不存在」,
    /// 而那种失败在界面上表现为「按钮点了没反应」,极难联想到是后端漏了
    #[test]
    fn 新加的三个命令得注册上() {
        // **注册表是 `.setup()` 的兄弟节点,不在它里面。** 拿 setup 那段去
        // 找命令名,永远找不到——第一条写这么查的测试就是这么假绿的
        for cmd in ["set_clip_body", "clip_source_url", "library_status"] {
            assert!(
                prod_source().contains(&format!("fn {cmd}(")),
                "{cmd} 没有实现"
            );
            assert!(
                prod_source().contains(&format!("\n            {cmd},")),
                "{cmd} 没在 generate_handler 里注册,前端调不到"
            );
        }
    }

    /// **写盘的方法清单里得把 `set_body` 算进去。**
    ///
    /// 那张清单是「每个会写盘的方法都记得自己写过」那条的检查依据。
    /// 漏了 `set_body` 的话,补全文会紧跟着触发一次全量重扫——
    /// 而用户看到的是「点一下补全,整个列表跳了一下」
    #[test]
    fn 补全文也算写盘() {
        // `set_body` 在 vault.rs 里,不在 lib.rs
        let src = include_str!("vault.rs");
        let body = body_of(src, "pub fn set_body");
        assert!(
            body.contains("self.note_write()"),
            "set_body 写了盘却没记,用户补一次全文就跟着一次全量重扫"
        );
    }

    /// **最近搜索得真的存得进去、读得出来。**
    ///
    /// 两个方向都要:命令在不在注册表里,界面调的那些名字对不对得上。
    /// 少一边就是"测试全绿,功能没有"——用户搜不到时看到的那一栏永远是空的
    #[test]
    fn 最近搜索得存得进读得出() {
        let setup = body_of(prod_source(), ".setup(|app|");
        for cmd in ["remember_search", "recent_searches"] {
            assert!(
                prod_source().contains(&format!("{cmd},")),
                "{cmd} 没注册,前端调不到"
            );
            assert!(
                setup.contains(cmd) || prod_source().contains(&format!("fn {cmd}")),
                "{cmd} 没有实现"
            );
        }
    }

    /// 索引也得有地方放。两个方向都要:
    /// 状态里得有槽位,还得真的在构造时给了初值
    #[test]
    fn 索引的存放位置接上了() {
        assert!(
            prod_source().contains("search_index: Mutex<HashMap<PathBuf, crate::index::Index>>"),
            "AppState 里没有索引槽位"
        );
        let setup = body_of(prod_source(), ".setup(|app|");
        assert!(
            setup.contains("search_index: Mutex::new(HashMap::new())"),
            "构造 AppState 时没给索引槽位初值"
        );
    }

    /// 索引文件**不能放在剪藏库里**。
    ///
    /// 剪藏库多半开着网盘/同步盘,而 SQLite 放进同步目录是冲突重灾区:
    /// 两台机器各改一份,同步下来变成两个文件,再下一轮就是互相覆盖,
    /// 最后索引整个坏掉。所以它必须待在应用自己的配置目录里
    #[test]
    fn 索引不进剪藏库() {
        let body = body_of(prod_source(), "fn index_for");
        assert!(
            body.contains("state.config_dir.join(\"indices\")"),
            "索引文件跟着剪藏库走了,会跟着同步软件打架"
        );
        assert!(
            !body.contains("clips_dir().join(\"index"),
            "索引文件被放进剪藏目录里了"
        );
    }

    /// 索引路径的哈希得稳定。同一个剪藏库两次启动得落在同一个文件上,
    /// 否则每次启动都从零重建,而且旧的还赖着不走
    #[test]
    fn 索引文件名对同一个目录是稳定的() {
        let a = index_file_name(Path::new("D:/剪藏/clips"));
        let b = index_file_name(Path::new("D:/剪藏/clips"));
        let c = index_file_name(Path::new("D:/剪藏/trash"));
        assert_eq!(a, b, "同一个目录两次算出两个名字");
        assert_ne!(a, c, "剪藏库和回收站撞到同一个索引上了");
        assert!(a.ends_with(".sqlite"));
    }


    /// **这一条是整个配置持久化的意义。** 重启一次不该要重选目录
    #[test]
    fn 记住的目录还在就接着用它() {
        let d = TempDir::new().unwrap();
        let saved = d.path().join("我的剪藏");
        std::fs::create_dir(&saved).unwrap();
        let cfg = settings::Settings {
            vault_path: Some(saved.clone()),
            ..Default::default()
        };

        let (dir, missing) = resolve_vault_dir(&cfg);

        assert_eq!(dir, saved);
        assert!(missing.is_none(), "目录还在就不该报'不见了'");
    }

    /// 第一次运行:没指过,用默认目录,**不该报任何问题**
    #[test]
    fn 没指过就用默认目录() {
        let cfg = settings::Settings::default();
        let (dir, missing) = resolve_vault_dir(&cfg);
        assert_eq!(dir, default_vault_dir());
        assert!(missing.is_none());
    }

    /// **这条最要紧。** 上次的目录没了(硬盘掉了、网盘没挂、同步盘换路径),
    /// 如果默默建一个新的空目录,用户看到"0 篇"就以为剪藏全没了——
    /// 而它们好好地躺在别处。必须说出来,而且说清是哪个路径
    #[test]
    fn 上次的目录不见了要说出来() {
        let d = TempDir::new().unwrap();
        let gone = d.path().join("拔了的移动硬盘").join("Quire");
        let cfg = settings::Settings {
            vault_path: Some(gone.clone()),
            ..Default::default()
        };

        let (dir, missing) = resolve_vault_dir(&cfg);

        assert_eq!(dir, gone, "还是用记录里那个路径,别偷偷换成别的地方");
        assert_eq!(missing, Some(gone.display().to_string()));
    }

    /// 目录不见了的时候**不能顺手把它建出来**。建出来就是空库,
    /// 而空库会让用户以为剪藏没了 —— 那比报个错糟得多
    #[test]
    fn 目录不见了不许重建() {
        let d = TempDir::new().unwrap();
        let gone = d.path().join("不在了");
        let cfg = settings::Settings {
            vault_path: Some(gone.clone()),
            ..Default::default()
        };
        let _ = resolve_vault_dir(&cfg);
        assert!(!gone.exists(), "resolve 不许有副作用");
    }

    /// 上次监控是开着的,启动就接着开。用户明确开过一次,
    /// 每次启动弹回"关"是在替他做决定
    #[test]
    fn 记住的监控开关会恢复() {
        let d = TempDir::new().unwrap();
        settings::Settings {
            watch_clipboard: true,
            ..Default::default()
        }
        .save(d.path())
        .unwrap();
        assert!(settings::Settings::load(d.path()).watch_clipboard);
    }

    /// 配置读不出来时,监控**必须是关的**。这是隐私承诺的落点:
    /// 宁可每次都让用户重开一次,也不能因为配置坏了就开始读剪贴板
    #[test]
    fn 配置坏掉时监控保持关() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join(settings::FILE_NAME), "{ 坏掉了").unwrap();
        assert!(!settings::Settings::load(d.path()).watch_clipboard);
    }


    /// 偶尔读不到不算坏。别的程序占一下剪贴板是常事,那时候弹红条
    /// 是在打扰一个没出任何问题的用户
    #[test]
    fn 偶尔读不到剪贴板不报警() {
        let state = WatchState::default();
        for _ in 0..(WATCH_FAIL_THRESHOLD - 1) {
            assert!(!state.note_read_failure(), "还没到阈值就不该报");
        }
    }

    /// 连着读不到就是坏了,得说。用户复制十篇一次不响,只会怀疑自己
    #[test]
    fn 连续读不到就该告诉用户() {
        let state = WatchState::default();
        let mut fired = 0;
        for _ in 0..WATCH_FAIL_THRESHOLD {
            if state.note_read_failure() {
                fired += 1;
            }
        }
        assert_eq!(fired, 1, "到阈值报一次就够");
    }

    /// 报完归零,不然红条会每 700ms 刷一遍。但故障持续下去时还会再报
    #[test]
    fn 报完之后要重新计() {
        let state = WatchState::default();
        for _ in 0..WATCH_FAIL_THRESHOLD {
            state.note_read_failure();
        }
        // 紧接着的失败还没攒够,不该再报
        assert!(!state.note_read_failure(), "报过一次之后要重新攒");
    }

    /// 中间成功一次就前功尽弃。抖一下不是坏
    #[test]
    fn 中间成功一次就重新计() {
        let state = WatchState::default();
        for _ in 0..(WATCH_FAIL_THRESHOLD - 1) {
            state.note_read_failure();
        }
        state.note_read_ok();
        for _ in 0..(WATCH_FAIL_THRESHOLD - 1) {
            assert!(!state.note_read_failure(), "成功过就该从头数");
        }
    }

    // ── 打开单篇文件 ──────────────────────────────────────────────
    //
    // 这组测试盯的是**安全闸**。`open_clip_file` 吃的是界面传进来的
    // 文件名,少了那道闸它就是个任意路径穿越:一个
    // `../../Windows/System32/xxx.md` 能让 Quire 变成任意文件的启动器。
    // 闸在哪儿、怎么判,是这组测试说了算

    /// 路径穿越一律拒绝。**每一种分隔符和父目录标记都要试**——
    /// 只测 `../` 的话,写的人改成 `..\` 就绕过去了
    #[test]
    fn 打开文件拒绝路径穿越() {
        // `vault` 在本文件里既是模块名也是类型名,别指望有个叫 `vault()`
        // 的辅助函数——那是从 vault.rs 的测试里抄来的,那边才有
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        for bad in [
            "../逃逸.md",
            "..\\逃逸.md",
            "clips/../逃逸.md",
            "/etc/passwd.md",
            "..",
            "a/b.md",
            "C:\\Windows\\x.md",
        ] {
            let got = clip_file_path(&v, bad).map_err(|e| e.wire().code);
            assert_eq!(
                got,
                Err("vault.unsafeFilename".to_string()),
                "「{bad}」该被拒绝,却放行了"
            );
        }
        drop(dir);
    }

    /// 不带 `.md` 后缀的也拒。**这不是多此一举**:用户能通过导入
    /// 往库里塞任何名字,而这个接口是拿文件名去拼路径的,
    /// 放行 `evil.exe` 就等于能执行它
    #[test]
    fn 打开文件只认md() {
        // `vault` 在本文件里既是模块名也是类型名,别指望有个叫 `vault()`
        // 的辅助函数——那是从 vault.rs 的测试里抄来的,那边才有
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        for bad in ["evil.exe", "shell.bat", "笔记", "a.md.exe"] {
            let got = clip_file_path(&v, bad).map_err(|e| e.wire().code);
            assert_eq!(got, Err("vault.unsafeFilename".to_string()), "「{bad}」该被拒");
        }
        drop(dir);
    }

    /// 名字合法但文件不在,要报「不存在」而不是「文件名不合法」——
    /// **两者的区别是用户下一步该做什么**:一个是重试,一个是找 bug
    #[test]
    fn 打开文件要分清不安全和不存在() {
        // `vault` 在本文件里既是模块名也是类型名,别指望有个叫 `vault()`
        // 的辅助函数——那是从 vault.rs 的测试里抄来的,那边才有
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let got = // 名字得用 ASCII:`is_safe_filename` 只放行字母数字和 `-_.`,
        // 所以"一个不存在的合法名字"得这么写,别用中文名
        clip_file_path(&v, "2026-01-01-x-not-a-real-clip.md").map_err(|e| e.wire().code);
        assert_eq!(got, Err("vault.notFound".to_string()));
        drop(dir);
    }

    /// 文件真在的时候,得**拼出剪藏库里的那个绝对路径**
    #[test]
    fn 打开文件拼出的是剪藏库里的路径() {
        // `vault` 在本文件里既是模块名也是类型名,别指望有个叫 `vault()`
        // 的辅助函数——那是从 vault.rs 的测试里抄来的,那边才有
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let saved = v.save(&ClipInput {
            schema_version: 1,
            url: "https://a.com".to_string(),
            title: "标题".to_string(),
            site_name: String::new(),
            author: None,
            excerpt: None,
            markdown: "正文".to_string(),
            published_at: None,
            image: None,
            favicon: None,
        })
        .unwrap();

        let path = clip_file_path(&v, &saved.filename).unwrap();

        assert_eq!(path, v.clips_dir().join(&saved.filename));
        assert!(path.is_file(), "拼出来的路径得真指着一个文件");
        drop(dir);
    }


    /// 启动回到上次那篇。**存了要有人读,读了要有人写**——
    /// 两个方向缺一个,这功能就是一段没人执行的死代码,而测试照样全绿
    #[test]
    fn 上次读到哪的两个方向都在() {
        let setter = body_of(prod_source(), "fn remember_last_read");
        assert!(
            setter.contains("cfg.last_read = Some(filename)"),
            "记了没存进配置"
        );
        let getter = body_of(prod_source(), "fn last_read(");
        assert!(
            getter.contains("cfg.last_read"),
            "存了没人读"
        );
    }

    /// 两条命令都得注册。**忘了注册不会编译失败**——它只是变成了一个
    /// 没人能从界面调到的函数
    #[test]
    fn 两条命令都注册了() {
        // **不能用 `body_of`**:它是给 `fn xxx() {` 那样配花括号的,
        // 而 `generate_handler![...]` 里一个花括号都没有,它会一路
        // 找到文件末尾然后报"没闭合"
        let start = SOURCE
            .find("tauri::generate_handler![")
            .expect("源码里找不到 generate_handler");
        let tail = &SOURCE[start..];
        let end = tail.find("];").expect("generate_handler 没收尾");
        let setup = &tail[..end];

        assert!(setup.contains("remember_last_read"), "remember_last_read 没注册");
        assert!(setup.contains("last_read,"), "last_read 没注册");
    }

    /// 记下来的那篇不在了,得**安静地**回列表而不是报错
    #[test]
    fn 记的那篇没了就回列表() {
        let body = body_of(prod_source(), "fn last_read(");
        assert!(
            body.contains("is_file().then_some(name)"),
            "文件不在了还返回它,界面上会打不开"
        );
        assert!(body.contains("?;"), "读配置失败不该当错误抛出去");
    }


    /// 到期的隔离文件得有人清。**没人调的话隔离区只进不出**,
    /// 一年之后它就是第二个剪藏库,用户还得自己想法子删
    #[test]
    fn 启动会清到期的隔离文件() {
        let setup = body_of(prod_source(), ".setup(|app|");
        assert!(setup.contains("sweep_deleted()"), "启动没清隔离区");
    }

    /// 真删那两条命令必须注册。**忘了注册不会编译失败**——
    /// 它只是变成了界面上够不到的两个函数
    #[test]
    fn 真删的命令注册了() {
        let start = SOURCE
            .find("tauri::generate_handler![")
            .expect("源码里找不到 generate_handler");
        let tail = &SOURCE[start..];
        let end = tail.find("];").expect("generate_handler 没收尾");
        let handler = &tail[..end];
        assert!(handler.contains("forget_clip,"), "forget_clip 没注册");
        assert!(handler.contains("forget_all,"), "forget_all 没注册");
    }

}
