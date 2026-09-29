//! 从系统剪贴板捕获剪藏素材。
//!
//! 走剪贴板而不是浏览器扩展,是因为后者要求用户先装扩展——对一个主打
//! 零门槛的工具来说,那是能劝退大多数人的一道坎。剪贴板是操作系统级的通道,
//! 不需要任何浏览器授权。
//!
//! Windows 上会同时读两个格式:
//! - `HTML Format`(`CF_HTML`):带结构的富文本,**并且带原文 URL**
//! - `CF_UNICODETEXT`:纯文本兜底
//!
//! URL 的价值在于回溯。少后读断了链接就等于断了根,拿不到原文地址的话,
//! 剪藏只能当一段孤立的文字。

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

/// 扩展往剪贴板里塞的元数据键。统一 `quire-` 前缀——用户随时可能复制任意网页,
/// 那些页面自带的 `<meta>` 一律不认,只认我们自己这一套。
pub const EXTENSION_META: &[&str] = &[
    "quire-version",
    "quire-title",
    "quire-site",
    "quire-author",
    "quire-excerpt",
    "quire-published",
    "quire-image",
];

/// 从系统剪贴板读到的原始素材,还没转成 Markdown。
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardCapture {
    pub html: Option<String>,
    pub text: String,
    /// 原文地址。CF_HTML 的 context 段里带,拿不到就留空让用户后补。
    pub url: Option<String>,
    /// 装了 Quire 扩展时由扩展附带;没装就是空的。
    /// 走剪贴板但没装扩展的用户,这里是空表——那是正常路径,不是故障。
    #[serde(default)]
    pub meta: HashMap<String, String>,
}

impl ClipboardCapture {
    /// 什么有效内容都没有时,别给用户一个空剪藏。
    pub fn is_empty(&self) -> bool {
        self.html.as_deref().map(str::trim).unwrap_or("").is_empty() && self.text.trim().is_empty()
    }
}

/// 从 `CF_HTML` 原始数据里切出 HTML、原文地址和扩展元数据。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCfHtml {
    /// context 段(完整可解析的 HTML 文档)
    pub html: String,
    pub source_url: Option<String>,
    pub meta: HashMap<String, String>,
}

/// 挑出扩展写的那些 meta 键。空值不收——`content=""` 等于没写,
/// 收下来只会让前端拿到一堆空字符串,还得挨个判空。
fn extract_meta(html: &str) -> HashMap<String, String> {
    let mut meta = HashMap::new();
    for key in EXTENSION_META {
        if let Some(value) = find_meta_content(html, key) {
            let value = value.trim();
            if !value.is_empty() {
                meta.insert((*key).to_string(), value.to_string());
            }
        }
    }
    meta
}

/// 解析头部的 `Key:Value` 行。遇到第一个不是这种形状的行就停——
/// 再往后就是 HTML 正文了。
fn parse_header(raw: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        let Some((key, value)) = line.split_once(':') else {
            break;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
            break;
        }
        map.insert(key.to_string(), value.trim().to_string());
    }
    map
}

/// 解析 `CF_HTML`。偏移是**字节**而不是字符,所以必须在字节切片上切,
/// 直接拿字符串索引会在正文含中文时错位。
pub fn parse_cf_html(raw: &str) -> Option<ParsedCfHtml> {
    let header = parse_header(raw);

    let start_html: i64 = header.get("StartHTML")?.parse().ok()?;
    let end_html: i64 = header.get("EndHTML")?.parse().ok()?;
    // -1 表示这份数据没带 context,没有可用偏移
    if start_html < 0 || end_html < 0 {
        return None;
    }
    let (start, end) = (start_html as usize, end_html as usize);
    let bytes = raw.as_bytes();
    if start >= end || end > bytes.len() {
        return None;
    }

    let html = String::from_utf8_lossy(&bytes[start..end]).into_owned();
    let source_url = extract_url(&header, &html);
    let meta = extract_meta(&html);
    Some(ParsedCfHtml {
        html,
        source_url,
        meta,
    })
}

/// 从三个可能的位置挖原文地址,按可靠性排序。
///
/// 不同程序往剪贴板里塞 URL 的习惯不一样:IE/MSHTML 用 `SourceURL` 头,
/// Chrome 用 `<meta http-equiv="refresh">`,有些用 `<base href>`,还有的塞
/// `<meta name="source-url">`。挨个兜住,少一个就可能整条回溯链断掉。
fn extract_url(header: &BTreeMap<String, String>, html: &str) -> Option<String> {
    let normalize = |u: &str| {
        let u = decode_html_entities(u.trim());
        if u.starts_with("http://") || u.starts_with("https://") {
            Some(u)
        } else {
            None
        }
    };

    if let Some(u) = header.get("SourceURL").and_then(|s| normalize(s)) {
        return Some(u);
    }
    if let Some(u) = find_meta_content(html, "source-url").and_then(|c| normalize(&c)) {
        return Some(u);
    }
    if let Some(u) = find_meta_refresh_url(html).and_then(|c| normalize(&c)) {
        return Some(u);
    }
    if let Some(u) = find_base_href(html).and_then(|c| normalize(&c)) {
        return Some(u);
    }
    None
}

/// `<meta http-equiv="refresh" content="0; url=https://...">`
fn find_meta_refresh_url(html: &str) -> Option<String> {
    let content = find_meta_content(html, "refresh")?;
    // content 形如 "0; url=https://example.com/a" 或 "5"
    let after = content.split_once("url=")?.1;
    Some(after.trim().trim_matches(['"', '\'']).to_string())
}

/// `<meta name="source-url" content="...">`
fn find_meta_content(html: &str, key: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let needle = format!("\"{key}\"");
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find(&needle) {
        let abs = from + rel;
        // 确认命中的是 <meta> 标签内的属性,别被正文里的同名字段骗了
        let tag_start = lower[..abs].rfind('<').unwrap_or(0);
        if lower[tag_start..abs].contains("<meta") {
            if let Some(v) = find_attr(&html[abs..], "content") {
                return Some(v);
            }
        }
        from = abs + needle.len();
    }
    None
}

/// `<base href="https://example.com/">`——官方文档说 context 段可以用它承载
/// 非绝对 URI 的解析基准,不少程序就是这么顺手把原文地址带上的。
fn find_base_href(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let pos = lower.find("<base")?;
    find_attr(&html[pos..], "href")
}

/// 在 `name=value` / `name="value"` 里取值。前一个字符必须是空白或引号,
/// 否则 `data-content=` 这类会误命中。
fn find_attr(s: &str, name: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    let needle = format!("{name}=");
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find(&needle) {
        let abs = from + rel;
        if abs > 0 {
            let prev = lower.as_bytes()[abs - 1];
            if !matches!(prev, b' ' | b'\t' | b'\r' | b'\n' | b'"' | b'\'') {
                from = abs + needle.len();
                continue;
            }
        }
        let rest = &s[abs + needle.len()..];
        let first = rest.chars().next()?;
        if first == '"' || first == '\'' {
            let end = rest[1..].find(first)?;
            return Some(rest[1..1 + end].to_string());
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(rest.len());
        return Some(rest[..end].to_string());
    }
    None
}

/// HTML 实体反转义。只处理 URL 里真会出现的几种——`&` 不还原的话
/// 带查询参数的链接存下来就是坏的。
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

#[cfg(windows)]
mod platform {
    use super::ClipboardCapture;
    use std::ptr;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    };
    // GlobalFree 在 windows-sys 0.60 里挪到了 Foundation,不在 Memory。
    // 只有测试写剪贴板那条路用得到它,正式构建不需要,别让它空报个警告
    #[cfg(test)]
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    // CF_UNICODETEXT 住在 Ole 命名空间下,这看着别扭却是 Windows API 的既定事实。
    // 它被定义成 u16,但下面几个 API 收的是 u32,得显式转。
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    const CF_UNICODETEXT_U32: u32 = CF_UNICODETEXT as u32;

    // 只有测试会往剪贴板里写。单独引,免得正式构建报未使用
    #[cfg(test)]
    use windows_sys::Win32::System::DataExchange::SetClipboardData;

    /// 这台机器的剪贴板现在能不能打开。只给测试用。
    ///
    /// 放这里的理由:`OpenClipboard` 是本模块的私有 import,测试那边拿不到;
    /// 与其把 import 改成 `pub use`(等于把 unsafe 边界扩散出去),
    /// 不如把这一个判断收在这里。
    #[cfg(test)]
    pub fn is_available() -> bool {
        unsafe {
            if OpenClipboard(ptr::null_mut()) == 0 {
                return false;
            }
            CloseClipboard();
            true
        }
    }

    /// 打开剪贴板并取走需要的两种格式。
    ///
    /// 剪贴板是全局共享资源,打开失败是常态(别的程序正占着),
    /// 所以重试几次而不是直接放弃。
    fn read_clipboard() -> Option<ClipboardCapture> {
        const RETRIES: u32 = 5;
        for attempt in 0..RETRIES {
            // 剪贴板是全进程共享的资源,这几个调用必须手写 unsafe。
            // 唯一需要守住的边界是"open 之后必定 close",这里用直白的控制流保证
            let opened = unsafe { OpenClipboard(ptr::null_mut()) } != 0;
            if opened {
                let capture = read_formats();
                unsafe { CloseClipboard() };
                return Some(capture);
            }
            if attempt + 1 < RETRIES {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        None
    }

    fn read_formats() -> ClipboardCapture {
        let mut capture = ClipboardCapture::default();

        unsafe {
            // PCWSTR 是 *const u16,得自己把格式名编成 UTF-16 并补 NUL 结尾。
            // 不能拿 c"..." 字面量硬转:那是 UTF-8 字节,只有纯 ASCII 时才碰巧对
            let mut name: Vec<u16> = "HTML Format".encode_utf16().collect();
            name.push(0);
            let html_format = RegisterClipboardFormatW(name.as_ptr());
            if html_format != 0 && IsClipboardFormatAvailable(html_format) != 0 {
                if let Some(raw) = read_global_bytes(GetClipboardData(html_format)) {
                    let text = String::from_utf8_lossy(&raw);
                    if let Some(parsed) = super::parse_cf_html(&text) {
                        capture.url = parsed.source_url;
                        capture.html = Some(parsed.html);
                        capture.meta = parsed.meta;
                    }
                }
            }

            if IsClipboardFormatAvailable(CF_UNICODETEXT_U32) != 0 {
                if let Some(text) = read_global_utf16(GetClipboardData(CF_UNICODETEXT_U32)) {
                    capture.text = text;
                }
            }
        }

        capture
    }

    /// `CF_HTML` 是 UTF-8(Windows API 少见的例外,其余文本格式都是 UTF-16)。
    unsafe fn read_global_bytes(handle: HANDLE) -> Option<Vec<u8>> {
        if handle.is_null() {
            return None;
        }
        let size = GlobalSize(handle);
        if size == 0 {
            return None;
        }
        let ptr = GlobalLock(handle);
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr as *const u8, size).to_vec();
        GlobalUnlock(handle);
        Some(bytes)
    }

    unsafe fn read_global_utf16(handle: HANDLE) -> Option<String> {
        if handle.is_null() {
            return None;
        }
        let size = GlobalSize(handle);
        if size < 2 {
            return None;
        }
        let ptr = GlobalLock(handle);
        if ptr.is_null() {
            return None;
        }
        let units = std::slice::from_raw_parts(ptr as *const u16, size / 2);
        // 剪贴板字符串以 NUL 结尾,取到第一个 NUL 为止
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..end]);
        GlobalUnlock(handle);
        Some(text)
    }

    pub fn capture() -> Option<ClipboardCapture> {
        read_clipboard()
    }

    /// 测试用:把一份 CF_HTML 原样写进真实剪贴板。**返回有没有真写进去。**
    ///
    /// 这条返回值不是多余的:剪贴板是全局共享资源,别的程序正占着的时候
    /// `OpenClipboard` 连着几次都会失败。之前这里打不开就静默 `return`,
    /// 测试接着读回来拿到的是上一次的残留内容,报出来的却是
    /// 「`assertion left == right failed`」——把环境占用说成了代码错误,
    /// 排查的人得先怀疑半天自己的代码。写没写成必须让调用方知道。
    #[cfg(test)]
    pub fn set_html_for_test(raw: &[u8]) -> bool {
        use windows_sys::Win32::System::DataExchange::EmptyClipboard;
        use windows_sys::Win32::System::Memory::{GlobalAlloc, GMEM_MOVEABLE};

        unsafe {
            let mut name: Vec<u16> = "HTML Format".encode_utf16().collect();
            name.push(0);
            let html_format = RegisterClipboardFormatW(name.as_ptr());

            if !open_with_retry() {
                return false;
            }
            EmptyClipboard();
            let hmem = GlobalAlloc(GMEM_MOVEABLE, raw.len());
            if hmem.is_null() {
                CloseClipboard();
                return false;
            }
            let dst = GlobalLock(hmem);
            if dst.is_null() {
                GlobalFree(hmem);
                CloseClipboard();
                return false;
            }
            std::ptr::copy_nonoverlapping(raw.as_ptr(), dst as *mut u8, raw.len());
            GlobalUnlock(hmem);
            // 所有权移交给系统,之后不能再解锁或释放这块内存
            let ok = !SetClipboardData(html_format, hmem as HANDLE).is_null();
            CloseClipboard();
            ok
        }
    }

    /// 测试用:记下剪贴板现状,好让测试跑完还原。
    ///
    /// 两种格式都存**原始字节**而不是解析结果——还原要的是用户原来的剪贴板
    /// 原封不动,只还原纯文本的话,用户往 Word 里粘贴会丢掉富文本格式。
    #[cfg(test)]
    pub struct Snapshot {
        html: Option<Vec<u8>>,
        text: Option<String>,
    }

    #[cfg(test)]
    impl Snapshot {
        pub fn take() -> Self {
            use windows_sys::Win32::System::DataExchange::EmptyClipboard;

            let mut snapshot = Snapshot {
                html: None,
                text: None,
            };
            // **必须先把剪贴板打开再读。** `OpenClipboard` 成功之后别的进程就
            // 改不动它,`GetClipboardData` 拿到的句柄在整个读的过程中都有效。
            //
            // 不开就读是个真会踩内存的错:浏览器之类随时可能 `EmptyClipboard`,
            // 句柄当场失效,后面 `GlobalSize` 拿到的是垃圾长度,再拿它
            // `from_raw_parts` 就是越界读写。表现是"单跑绿、全跑红",
            // 还夹着一次堆损坏退出——以前一直当成是环境问题,其实是自己写的。
            if !open_with_retry() {
                return snapshot;
            }
            unsafe {
                let mut name: Vec<u16> = "HTML Format".encode_utf16().collect();
                name.push(0);
                let html_format = RegisterClipboardFormatW(name.as_ptr());
                if html_format != 0 && IsClipboardFormatAvailable(html_format) != 0 {
                    snapshot.html = read_global_bytes(GetClipboardData(html_format));
                }
                if IsClipboardFormatAvailable(CF_UNICODETEXT_U32) != 0 {
                    snapshot.text = read_global_utf16(GetClipboardData(CF_UNICODETEXT_U32));
                }
                EmptyClipboard();
                CloseClipboard();
            }
            snapshot
        }

        pub fn restore(self) {
            use windows_sys::Win32::System::DataExchange::EmptyClipboard;
            use windows_sys::Win32::System::Memory::{GlobalAlloc, GMEM_MOVEABLE};

            unsafe {
                let mut name: Vec<u16> = "HTML Format".encode_utf16().collect();
                name.push(0);
                let html_format = RegisterClipboardFormatW(name.as_ptr());
                if !open_with_retry() {
                    return;
                }
                EmptyClipboard();
                for (format, bytes) in [
                    (
                        CF_UNICODETEXT_U32,
                        self.text.map(|t| {
                            let mut u: Vec<u16> = t.encode_utf16().collect();
                            u.push(0);
                            let mut raw = Vec::new();
                            for c in u {
                                raw.extend_from_slice(&c.to_ne_bytes());
                            }
                            raw
                        }),
                    ),
                    (html_format, self.html),
                ] {
                    let Some(bytes) = bytes else { continue };
                    if bytes.is_empty() {
                        continue;
                    }
                    let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
                    if hmem.is_null() {
                        continue;
                    }
                    let dst = GlobalLock(hmem);
                    if dst.is_null() {
                        continue;
                    }
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst as *mut u8, bytes.len());
                    GlobalUnlock(hmem);
                    SetClipboardData(format, hmem as HANDLE);
                }
                CloseClipboard();
            }
        }
    }

    #[cfg(test)]
    fn open_with_retry() -> bool {
        // 10 次 × 30ms ≈ 300ms。剪贴板被占通常是"别的程序正在复制"这种
        // 转瞬即逝的事,80ms(5 × 20ms)偏短,实测会漏掉一部分
        for attempt in 0..10 {
            if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
                return true;
            }
            if attempt < 9 {
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        }
        false
    }
}

/// macOS:从 NSPasteboard 读。
///
/// 和 Windows 那边不是同一套东西,差别集中在两点:
///
/// 1. **格式名不一样。** Windows 是自定义的 `HTML Format` 加一段带字节偏移的
///    头部;macOS 直接就是 UTI `public.html`,拿到的是干净的 HTML 片段,
///    没有偏移要算。所以走的是 [`super::extract_meta`] 之外的路径——元数据
///    仍然从 HTML 里的 `<meta name="quire-*">` 取,和扩展的约定保持一致。
/// 2. **地址是独立的一份。** macOS 放在 `public.url` 这个 UTI 上,直接从
///    剪贴板取,不用像 Windows 那样在 HTML 头部里翻 `SourceURL`。
///
/// ⚠️ **这段代码没有在 macOS 上编译或运行过**——写它的那台机器是 Windows。
/// API 签名是对着 objc2-app-kit 0.3.2 的源码核过的,但"能编译"和"跑起来
/// 对"是两件事,尤其是 NSPasteboard 在不同 macOS 版本上的行为差异。
/// 发版前必须在真机上验证一遍,别直接信这段注释。
#[cfg(target_os = "macos")]
mod platform {
    // NSString 不用单独引:`stringForType` 返回 `Retained<NSString>`,而
    // `to_string()` 走的是 Display 实现,那部分不需要这个名字在作用域里。
    // 之前多引了一个,clippy 在 macOS 上直接报 unused import。
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString, NSPasteboardTypeURL,
    };

    pub fn capture() -> Option<super::ClipboardCapture> {
        let board = NSPasteboard::generalPasteboard();

        // 这三个格式常量是 extern static,不是普通 const,取值必须显式 unsafe。
        // 编译器认这个:写漏了会直接报 E0133,不会等到运行时才炸。
        let html_type = unsafe { NSPasteboardTypeHTML };
        let string_type = unsafe { NSPasteboardTypeString };
        let url_type = unsafe { NSPasteboardTypeURL };

        let html: Option<String> = board.stringForType(html_type).map(|s| s.to_string());
        // text 是必填字段,拿不到就留空串——is_empty() 会按"什么都没有"
        // 处理掉,不会剪出一个空条目
        let text: String = board
            .stringForType(string_type)
            .map(|s| s.to_string())
            .unwrap_or_default();
        // public.url 存的是 URL 文本,两端可能带空白,直接存库会变成脏链接
        let url: Option<String> = board
            .stringForType(url_type)
            .map(|s| s.to_string())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        if html.is_none() && text.trim().is_empty() {
            return None;
        }

        // 和 Windows 保持同一个约定:meta 从 HTML 里取,所以复制粘贴网页
        // (只有 HTML、没有扩展写的 quire-* 标记)一样能拿到地址和正文。
        let meta = html.as_deref().map(super::extract_meta).unwrap_or_default();

        // 刻意不写 `..Default::default()`:字段全列出来了,那玩意儿是个空操作,
        // clippy 会报 "struct update has no effect"。而少写字段本来就是编译错误,
        // 以后 `ClipboardCapture` 加了字段,这里会当场编不过——想要的安全网
        // 本来就不需要它。
        Some(super::ClipboardCapture {
            url,
            html,
            text,
            meta,
        })
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::ClipboardCapture;

    /// Linux 和别的平台目前给不出内容。与其给个半残的实现,不如在界面上
    /// 明说"当前平台尚未支持"——Linux 上的剪贴板 HTML 得走 X11/Wayland
    /// 两套完全不同的协议,值得单独做,但不跟 macOS 一起塞进来。
    pub fn capture() -> Option<ClipboardCapture> {
        None
    }
}

/// 读不到就是读不到,**不编一句中文给界面**——界面自己查 `clipboard.unavailable`
/// 那条文案,英文用户看见的也该是英文。
pub fn capture_clipboard() -> Result<ClipboardCapture, crate::vault::WireError> {
    platform::capture().ok_or_else(|| crate::vault::WireError::new("clipboard.unavailable"))
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    /// 拼一份符合微软规范样例的 CF_HTML,偏移量自动算对。
    /// 真实剪贴板数据的偏移就是这么来的,手写死数字很容易错——
    /// 换行符一变(CRLF→LF)每个偏移都会差,所以换行符也做成参数。
    fn build_cf_html_nl(nl: &str, header_extra: &[(&str, &str)], context: &str) -> String {
        let mut header = String::from("Version:1.0");
        header.push_str(nl);
        header.push_str("StartHTML:0000000000");
        header.push_str(nl);
        header.push_str("EndHTML:0000000000");
        header.push_str(nl);
        header.push_str("StartFragment:0000000000");
        header.push_str(nl);
        header.push_str("EndFragment:0000000000");
        header.push_str(nl);
        for (k, v) in header_extra {
            header.push_str(&format!("{k}:{v}"));
            header.push_str(nl);
        }
        let html = context;
        let start = header.len();
        let end = start + html.len();
        // 按规范可左填充零,用十位对齐真实剪贴板的写法
        let header = header
            .replace("StartHTML:0000000000", &format!("StartHTML:{start:010}"))
            .replace("EndHTML:0000000000", &format!("EndHTML:{end:010}"))
            .replace(
                "StartFragment:0000000000",
                &format!("StartFragment:{start:010}"),
            )
            .replace("EndFragment:0000000000", &format!("EndFragment:{end:010}"));
        format!("{header}{html}")
    }

    fn build_cf_html(header_extra: &[(&str, &str)], context: &str) -> String {
        build_cf_html_nl("\r\n", header_extra, context)
    }

    const CONTEXT: &str = "<html><body><!--StartFragment--><h1>标题</h1><p>正文内容</p><!--EndFragment--></body></html>";

    /// 这台机器的剪贴板现在能不能用。
    ///
    /// 下面两条往返测试**要求独占系统剪贴板**。CI 的 runner、锁屏的桌面、
    /// 或者任何正占着剪贴板的程序,都会让 `OpenClipboard` 连续失败。这时候
    /// 测试红,但红的原因和代码无关——`clipboard.rs` 一个字都没错。
    ///
    /// 所以要先探一下,分清「环境不给用」和「读回来的数据不对」。混在一起
    /// 报,时间久了就没人看红灯是哪个原因了。
    ///
    /// 真要在本机强制要求它可用(不想让 CI 悄悄跳过),设
    /// `QUIRE_REQUIRE_CLIPBOARD=1`,不可用就直接失败。
    #[cfg(windows)]
    fn clipboard_available() -> bool {
        platform::is_available()
    }

    /// 剪贴板不可用就跳过,并把原因打在输出里。**不静默跳过**——
    /// 静默跳过等于假装这条测试跑过了,那是比红灯更糟的失败。
    #[cfg(windows)]
    fn require_clipboard(test_name: &str) -> bool {
        if clipboard_available() {
            return true;
        }
        let forced = std::env::var("QUIRE_REQUIRE_CLIPBOARD").is_ok();
        let reason = format!(
            "{test_name}:本机剪贴板当前不可用(多半是别的程序占着,或 CI 的无人值守会话),\
             这不是代码问题。设 QUIRE_REQUIRE_CLIPBOARD=1 可要求它必须可用。"
        );
        if forced {
            panic!("{reason}");
        }
        eprintln!("跳过 —— {reason}");
        false
    }

    /// 读回来的还是不是**我们写进去的那一份**。
    ///
    /// 剪贴板是全局共享的,写和读之间任何一个程序(浏览器、编辑器、
    /// 连字取巧的小工具)动了它,我们读到的就是别人的东西。而报出来的是
    /// 「元数据应齐全: {}」——把环境抢占说成了解析坏了,排查的人会去翻
    /// 自己的解析代码,方向从第一步就错了。
    ///
    /// `canary` 是只出现在我们写的那份里的字样,拿它当判据。
    fn require_ours(capture: &ClipboardCapture, canary: &str, test_name: &str) -> bool {
        if capture.html.as_deref().is_some_and(|h| h.contains(canary)) {
            return true;
        }
        let forced = std::env::var("QUIRE_REQUIRE_CLIPBOARD").is_ok();
        let reason = format!(
            "{test_name}:写完之后读回来的已经不是我们写的那份了(别的程序在这中间              动了剪贴板)。这是环境问题,不是这条链路坏了。设 QUIRE_REQUIRE_CLIPBOARD=1              可要求它必须原样回来。"
        );
        if forced {
            panic!("{reason}");
        }
        eprintln!("跳过 —— {reason}");
        false
    }

    /// 写进真实剪贴板之后确认一下。**写不进去要按"环境占着"处理,
    /// 不能报成断言失败**——测试接着读回来拿到的是上一次的残留内容,
    /// 报出来的是「`left == right` failed」,把环境问题说成了代码问题,
    /// 排查的人得先怀疑半天自己的代码,方向从一开始就错了。
    fn require_written(written: bool, test_name: &str) -> bool {
        if written {
            return true;
        }
        let forced = std::env::var("QUIRE_REQUIRE_CLIPBOARD").is_ok();
        let reason = format!(
            "{test_name}:没能写进真实剪贴板(别的程序正占着它,或 CI 的无人值守会话)。             这是环境问题,不是这条链路坏了。设 QUIRE_REQUIRE_CLIPBOARD=1 可要求它必须成功。"
        );
        if forced {
            panic!("{reason}");
        }
        eprintln!("跳过 —— {reason}");
        false
    }

    #[test]
    fn 切出context并保留StartFragment标记() {
        let raw = build_cf_html(&[], CONTEXT);
        let parsed = parse_cf_html(&raw).expect("应能解析");
        assert_eq!(parsed.html, CONTEXT);
        assert!(parsed.html.contains("StartFragment"));
    }

    #[test]
    fn 从SourceURL头取地址() {
        let raw = build_cf_html(&[("SourceURL", "https://example.com/post/1")], CONTEXT);
        let parsed = parse_cf_html(&raw).unwrap();
        assert_eq!(
            parsed.source_url.as_deref(),
            Some("https://example.com/post/1")
        );
    }

    #[test]
    fn 从meta刷新取地址() {
        // Chrome 往剪贴板里塞原文地址用的就是这招,不是 SourceURL 头
        let ctx = format!("<html><head><meta http-equiv=\"refresh\" content=\"0; url=https://news.example.com/a\"></head><body>{CONTEXT}</body></html>");
        let raw = build_cf_html(&[], &ctx);
        let parsed = parse_cf_html(&raw).unwrap();
        assert_eq!(
            parsed.source_url.as_deref(),
            Some("https://news.example.com/a")
        );
    }

    #[test]
    fn 从base_href取地址() {
        let ctx = format!("<html><head><base href=\"https://blog.example.com/post/\"></head><body>{CONTEXT}</body></html>");
        let raw = build_cf_html(&[], &ctx);
        let parsed = parse_cf_html(&raw).unwrap();
        assert_eq!(
            parsed.source_url.as_deref(),
            Some("https://blog.example.com/post/")
        );
    }

    #[test]
    fn 从meta_source_url取地址() {
        let ctx = format!("<html><head><meta name=\"source-url\" content=\"https://s.example.com/x\"></head><body>{CONTEXT}</body></html>");
        let raw = build_cf_html(&[], &ctx);
        let parsed = parse_cf_html(&raw).unwrap();
        assert_eq!(
            parsed.source_url.as_deref(),
            Some("https://s.example.com/x")
        );
    }

    #[test]
    fn 地址里的HTML实体被还原() {
        // 带查询参数的链接里 & 会被编码成 &amp;,不还原就是条坏链
        let raw = build_cf_html(
            &[("SourceURL", "https://example.com/s?a=1&amp;b=2")],
            CONTEXT,
        );
        let parsed = parse_cf_html(&raw).unwrap();
        assert_eq!(
            parsed.source_url.as_deref(),
            Some("https://example.com/s?a=1&b=2")
        );
    }

    #[test]
    fn 正文含中文时偏移仍然正确() {
        // 偏移是字节数。中文在 UTF-8 里占 3 字节,拿字符索引切就会错位——
        // 这条测试是整个模块最容易悄悄坏掉的地方
        let ctx = format!("<html><body><!--StartFragment--><p>中文正文,占三个字节一个字符</p>{CONTEXT}</body></html>");
        let raw = build_cf_html(&[], &ctx);
        let parsed = parse_cf_html(&raw).expect("应能解析");
        assert!(parsed.html.starts_with("<html>"));
        assert!(parsed.html.ends_with("</html>"));
        assert!(parsed.html.contains("中文正文"));
    }

    #[test]
    fn LF换行的header也能解析() {
        // 规范说换行可能是 CRLF、LF 或孤立的 CR,三种都得认
        let raw = build_cf_html_nl("\n", &[], CONTEXT);
        assert!(!raw.contains('\r'), "构造的数据就该是纯 LF");
        let parsed = parse_cf_html(&raw).unwrap();
        assert_eq!(parsed.html, CONTEXT);
    }

    #[test]
    fn 负偏移视为无context() {
        let raw = "Version:1.0\r\nStartHTML:-1\r\nEndHTML:-1\r\n<!--StartFragment-->正文<!--EndFragment-->";
        assert!(parse_cf_html(raw).is_none());
    }

    #[test]
    fn 偏移越界不panic() {
        // 剪贴板内容被别的前后截断是可能的,不能因此崩掉整个应用
        let raw = "Version:1.0\r\nStartHTML:0000000100\r\nEndHTML:0000009999\r\n短";
        assert!(parse_cf_html(raw).is_none());
    }

    #[test]
    fn 缺header的垃圾数据返回None() {
        assert!(parse_cf_html("").is_none());
        assert!(parse_cf_html("随便一段不是剪贴板格式的文字").is_none());
    }

    #[test]
    fn 正文里的同名字段不会冒充地址() {
        // 正文里出现 `<meta name="source-url">` 只在 meta 标签内才认
        let ctx = "<html><body><p>提到 source-url 这个词</p><span>source-url: 我不是地址</span></body></html>";
        let raw = build_cf_html(&[], ctx);
        let parsed = parse_cf_html(&raw).unwrap();
        assert_eq!(parsed.source_url, None);
    }

    #[test]
    fn 抓不到地址时不编造() {
        let raw = build_cf_html(&[], CONTEXT);
        let parsed = parse_cf_html(&raw).unwrap();
        assert_eq!(
            parsed.source_url, None,
            "没有就是没有,不能拿正文里的东西凑一个"
        );
    }

    #[test]
    fn 非HTTP地址被丢弃() {
        // javascript: 和 data: 拼进 frontmatter 是安全隐患
        let raw = build_cf_html(&[("SourceURL", "javascript:alert(1)")], CONTEXT);
        assert_eq!(parse_cf_html(&raw).unwrap().source_url, None);
    }

    /// 扩展与桌面端之间唯一的约定就是这组 meta 键。两边同时改键名不会
    /// 报任何编译错误,只会让用户装完扩展发现"字段还是空的",所以钉死。
    fn extension_html(metas: &[(&str, &str)]) -> String {
        let head: String = metas
            .iter()
            .map(|(k, v)| format!("<meta name=\"{k}\" content=\"{v}\">"))
            .collect();
        format!("<html><head>{head}</head><body>{CONTEXT}</body></html>")
    }

    #[test]
    fn 扩展元数据能被读出来() {
        let ctx = extension_html(&[
            ("quire-version", "1"),
            ("quire-title", "深入理解所有权"),
            ("quire-site", "Rust 官方文档"),
            ("quire-author", "张三"),
            ("quire-excerpt", "所有权是 Rust 最核心的概念之一。"),
            ("quire-published", "2026-09-20T08:00:00+08:00"),
            ("quire-image", "https://example.com/cover.png"),
        ]);
        let parsed = parse_cf_html(&build_cf_html(&[], &ctx)).unwrap();
        let meta = &parsed.meta;

        assert_eq!(meta.len(), EXTENSION_META.len(), "七个键都该读到: {meta:?}");
        assert_eq!(meta["quire-title"], "深入理解所有权");
        assert_eq!(meta["quire-site"], "Rust 官方文档");
        assert_eq!(meta["quire-author"], "张三");
        assert_eq!(meta["quire-image"], "https://example.com/cover.png");
    }

    #[test]
    fn 没装扩展时元数据是空的() {
        // 这是绝大多数用户的正常路径,不能因为 meta 为空就当成错误
        let parsed = parse_cf_html(&build_cf_html(&[], CONTEXT)).unwrap();
        assert!(parsed.meta.is_empty(), "普通复制不该有扩展元数据");
    }

    #[test]
    fn 网页自带的meta不会被误认成扩展元数据() {
        // 用户复制的任意页面都可能有 author / description 这类 meta,
        // 不加 quire- 前缀就会把它们当成扩展填的元数据,用户看到作者栏
        // 凭空冒出个东西却不知道哪来的
        let ctx = format!(
            "<html><head><meta name=\"author\" content=\"路人\"><meta name=\"description\" content=\"简介\"></head><body>{CONTEXT}</body></html>"
        );
        let parsed = parse_cf_html(&build_cf_html(&[], &ctx)).unwrap();
        assert!(parsed.meta.is_empty(), "非 quire- 前缀的一律不认");
    }

    #[test]
    fn 空的meta值不收() {
        // content="" 等于没写,收下来前端还得挨个判空
        let ctx = extension_html(&[("quire-title", "   ")]);
        let parsed = parse_cf_html(&build_cf_html(&[], &ctx)).unwrap();
        assert!(parsed.meta.is_empty());
    }

    #[test]
    fn 扩展元数据与原文地址可以同时拿到() {
        // 扩展是靠 source-url 那个 meta 带地址的,这条路径必须和
        // 扩展元数据共存,否则装扩展反而丢了回溯能力
        let ctx = extension_html(&[
            ("source-url", "https://example.com/p"),
            ("quire-title", "标题"),
        ]);
        let parsed = parse_cf_html(&build_cf_html(&[], &ctx)).unwrap();
        assert_eq!(parsed.source_url.as_deref(), Some("https://example.com/p"));
        assert_eq!(parsed.meta["quire-title"], "标题");
    }

    /// 真实剪贴板往返测试的守卫。跑完(哪怕是断言 panic 导致的提前退出)
    /// 都会把用户原本的剪贴板还原回去——测试不该在用户机器上留副作用。
    #[cfg(windows)]
    struct ClipboardGuard(Option<platform::Snapshot>);

    #[cfg(windows)]
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            if let Some(snapshot) = self.0.take() {
                snapshot.restore();
            }
        }
    }

    /// 剪贴板是**全机器唯一**的资源,cargo 默认又并行跑测试。
    /// 两条往返测试不串行就会互相把对方刚写的数据盖掉,表现为
    /// "单跑绿、全跑红",而且红的行号每次还不一样。必须排队。
    #[cfg(windows)]
    static CLIPBOARD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(windows)]
    fn lock_clipboard() -> std::sync::MutexGuard<'static, ()> {
        CLIPBOARD_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 前面 14 条测的都是纯函数:给一段字符串,看解析对不对。
    /// 但真正决定这个软件能不能用的,是 `platform` 里那套 Win32 调用——
    /// 格式名拼错、句柄类型搞混、GlobalSize 拿到的长度不对,上面全绿也照样
    /// 读不出任何东西。这条测试把数据写进**真实剪贴板**再读回来,
    /// 是唯一能证明这条链路真的通的测试。
    #[cfg(windows)]
    #[test]
    fn 真实剪贴板往返() {
        use super::platform;

        if !require_clipboard("真实剪贴板往返") {
            return;
        }

        let _serial = lock_clipboard();
        let guard = ClipboardGuard(Some(platform::Snapshot::take()));

        // 按 Chrome 的真实排版来:CRLF 换行、Version:0.9、正文里带中文
        let context = "<html>\r\n<body>\r\n<!--StartFragment--><h1>深入理解所有权</h1>\
            <p>所有权是 Rust 最核心的概念之一。</p><!--EndFragment-->\r\n</body>\r\n</html>";
        let raw = build_cf_html_nl(
            "\r\n",
            &[("SourceURL", "https://example.com/post/1")],
            context,
        );
        if !require_written(platform::set_html_for_test(raw.as_bytes()), "真实剪贴板往返") {
            return;
        }

        let capture = capture_clipboard().expect("应能读回刚写进去的内容");
        if !require_ours(&capture, "深入理解所有权", "真实剪贴板往返") {
            return;
        }
        assert_eq!(capture.url.as_deref(), Some("https://example.com/post/1"));
        assert_eq!(capture.html.as_deref(), Some(context), "HTML 应逐字节读回");
        assert!(!capture.is_empty());

        // 提前放行,让还原在断言全过之后立刻发生
        drop(guard);
    }

    /// 扩展载荷走真实剪贴板的往返。
    ///
    /// 纯函数测试已经证明 meta 能从 HTML 里解析出来,但那只覆盖了
    /// "给定这段字符串"。这里证明的是**扩展写的那份 context 段经过
    /// Windows 剪贴板往返之后,元数据和地址都还在**——中间隔着
    /// UTF-8 编码、GlobalSize 长度、零字节填充,任何一环出错都是
    /// 用户装完扩展发现"字段还是空的",而且只在真机上才暴露。
    #[cfg(windows)]
    #[test]
    fn 扩展载荷经真实剪贴板仍完整() {
        use super::platform;

        if !require_clipboard("扩展载荷经真实剪贴板仍完整") {
            return;
        }

        let _serial = lock_clipboard();
        let guard = ClipboardGuard(Some(platform::Snapshot::take()));

        let ctx = extension_html(&[
            ("quire-version", "1"),
            ("quire-title", "本地优先的稍后读"),
            ("quire-site", "Rust 官方文档"),
            ("quire-author", "张三"),
            ("quire-excerpt", "为什么我们还在用云端笔记?"),
            ("quire-published", "2026-09-20T08:00:00+08:00"),
            ("quire-image", "https://example.com/cover.png"),
            // 扩展靠这个 meta 带地址,和元数据必须同时活下来
            ("source-url", "https://example.com/post/1"),
        ]);
        let raw = build_cf_html_nl("\r\n", &[], &ctx);
        if !require_written(
            platform::set_html_for_test(raw.as_bytes()),
            "扩展载荷经真实剪贴板仍完整",
        ) {
            return;
        }

        let capture = capture_clipboard().expect("应能读回扩展载荷");
        if !require_ours(&capture, "本地优先的稍后读", "扩展载荷经真实剪贴板仍完整") {
            return;
        }
        assert_eq!(
            capture.meta.len(),
            EXTENSION_META.len(),
            "元数据应齐全: {:?}",
            capture.meta
        );
        assert_eq!(capture.meta["quire-title"], "本地优先的稍后读");
        assert_eq!(capture.meta["quire-site"], "Rust 官方文档");
        assert_eq!(capture.meta["quire-author"], "张三");
        assert_eq!(capture.meta["quire-excerpt"], "为什么我们还在用云端笔记?");
        assert_eq!(capture.meta["quire-published"], "2026-09-20T08:00:00+08:00");
        assert_eq!(capture.meta["quire-image"], "https://example.com/cover.png");
        assert_eq!(capture.url.as_deref(), Some("https://example.com/post/1"));

        drop(guard);
    }
}
