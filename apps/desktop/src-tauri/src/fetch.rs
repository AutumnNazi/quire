//! 自己去把文章页面抓下来。
//!
//! ## 为什么要有这条
//!
//! 之前只有浏览器扩展能存全文:扩展跑在页面里,DOM 现成,Defuddle 直接
//! 就能用。而扩展意味着**装 Quire 要两步**——装软件,再装扩展、授权。
//!
//! 这一步的门槛不是"多花三十秒"。真正劝退人的是那个心理成本:
//! 「为了存篇文章,我得让一个陌生软件进到我的浏览器里」。而这个软件
//! 已经在我的电脑上了——它凭什么还得先经过浏览器这一关?
//!
//! 所以这条路的意义是**让扩展从必需品降级成可选增强**:只存网页自己
//! 抓得到的部分(标题、作者、发布时间),扩展才带来截图、选区、整页原样。
//!
//! ## 为什么抓取在 Rust、正文抽取在 webview
//!
//! 正文抽取要 DOM,而 DOM 只存在于 webview 里。`reqwest` 拿 HTML,
//! 交给 webview 里的 Defuddle 抽正文——两边各自做自己擅长的,
//! **不引入第二套抽取实现**。
//!
//! ## 抓不到不是错
//!
//! 需要登录的页面、纯 JS 渲染的页面、抓取被拦的站点,都会失败。那是
//! **正常结局**,不是故障:用户复制的那段文字照样存得下来。界面上要说清
//! "抓不到全文,已存你复制的内容",而不是弹一个红条。

use std::time::Duration;

/// 抓下来的页面。**只存 HTML,不在这里抽正文**
#[derive(Debug, Clone, Default)]
pub struct Fetched {
    pub html: String,
    /// 最终的地址。页面可能 301 跳到别处,而正文里的相对链接要按它解析
    pub final_url: String,
}

/// 抓不到时的原因。**送代号不送句子**——这句话最终显示给用户看,
/// 而用户用哪种语言不归这个模块管
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// 地址不是 http(s)。`javascript:`、`file:`、`data:` 都走这里
    NotHttp(String),
    /// 抓下来了但不是 HTML——PDF、图片、纯 JSON
    NotHtml(String),
    /// 服务端或网络的问题。**要和「抓不到正文」分开**:
    /// 一个是暂时性的(重试可能成),另一个是页面本身的性质
    Network(String),
    /// 抓到了页面但没等来要的东西(超时)
    Timeout,
    /// 页面太大。整站转存的页面能有几十兆,塞进 webview 会卡住
    TooLarge(usize),
}

/// 超过这个大小就不抓了。**20MB 是硬闸不是软限**——webview 拿到一个
/// 30MB 的字符串要重新解析整个 DOM,那一下就能让界面假死好几秒
pub const MAX_HTML_BYTES: usize = 20 * 1024 * 1024;

/// 复制的内容短于这个字数才值得去抓全文。500 字大概是普通博客文章的长度。
///
/// **定义在这儿,前端也有一份同名的 `FULL_TEXT_MIN_CLIP_CHARS`。**
/// 两处都要用(剪藏那一刻前端判断值不值得抓;`library_status` 后端判断
/// 这一篇是不是当初没抓到),而各写一份的话迟早有一处被改漏——
/// 改漏的后果是"有的篇能补有的不能补",用户完全看不出规律
pub const MIN_CLIP_CHARS_FOR_FETCH: usize = 500;

/// 该不该试着手抓这个地址。
///
/// **先看形态,再动手。** 大部分剪藏是用户复制的一段话,地址是
/// `https://…` 的、能抓的;但也有 `file:///…`、编辑器里拖进来的、
/// 剪贴板里压根没链接的——那些一律不碰,省掉一次必然失败的网络往返
pub fn looks_fetchable(url: &str) -> bool {
    let u = url.trim();
    (u.starts_with("http://") || u.starts_with("https://")) && u.len() > 12
}

/// 真去抓。**会阻塞**,调用方得放到后台线程
pub fn fetch_page(url: &str) -> Result<Fetched, FetchError> {
    if !looks_fetchable(url) {
        return Err(FetchError::NotHttp(url.to_string()));
    }
    let client = build_client();
    let resp = client.get(url).send().map_err(|e| {
        if e.is_timeout() {
            FetchError::Timeout
        } else {
            FetchError::Network(e.to_string())
        }
    })?;

    let status = resp.status();
    if !status.is_success() && status.as_u16() != 304 {
        return Err(FetchError::Network(format!("HTTP {}", status.as_u16())));
    }
    let final_url = resp.url().to_string();

    // **先按响应头挡一次,再决定要不要把正文读进内存。**
    // 压缩是关掉的(见 Cargo.toml 的 features,没开 gzip/brotli/zstd),
    // 所以这里的 Content-Length 就是正文的真身大小。30MB 的页面在这一步
    // 就走了,不用先花几百毫秒读进来再扔掉——那道按实读的闸门在下面,
    // 它是给不给 Content-Length 的服务端兜底的
    if let Some(len) = declared_len(resp.headers()) {
        size_gate(len)?;
    }

    // **`text()` 按页面声明的编码解码**,它带了 charset feature,
    // 内部就是 encoding_rs——自己再引一份是同一个库编两遍,
    // 而中文站点里 GBK 仍然常见,一律按 UTF-8 解会得到一片乱码。
    let html = resp.text().map_err(|e| {
        if e.is_timeout() {
            FetchError::Timeout
        } else {
            FetchError::Network(e.to_string())
        }
    })?;
    size_gate(html.len())?;

    // **不是 HTML 就别送进 webview。** 一个 PDF 被塞进 DOMParser,
    // 抽出来的是一堆乱码当正文——比老实说"抓不到"糟得多
    if !looks_like_html(&html) {
        return Err(FetchError::NotHtml(content_type_of(&html).to_string()));
    }
    Ok(Fetched { html, final_url })
}

/// 超过上限就拦下来。**两道闸门共用它**——响应头那道和读完那道,
/// 判的是同一件事,写成两处的话早晚有一处被改漏
fn size_gate(n: usize) -> Result<(), FetchError> {
    if n > MAX_HTML_BYTES {
        return Err(FetchError::TooLarge(n));
    }
    Ok(())
}

/// 服务端报了多少字节。**报得不对就当没报。** 有几类服务端会把
/// `Content-Length` 写成 chunked 之外的样子,或者干脆给个垃圾值——
/// 宁可退回实读那道,也不能拿一个错数字把正常页面拦在门外
fn declared_len(headers: &reqwest::header::HeaderMap) -> Option<usize> {
    headers
        .get(reqwest::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn build_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        // **不压缩——靠的是 Cargo.toml 里的 features,不是这里。**
        // reqwest 的 `gzip`/`brotli`/`zstd` 一个都没开,客户端压根不发
        // `Accept-Encoding`。这不是可有可无的省事:开了压缩的话,
        // `Content-Length` 是压缩后的大小,而读到手里的是解压后的——
        // 大小闸门量到的就不是同一个东西,一个 2MB 的响应能解开成 200MB。
        // 真要开压缩,闸门就得改成读流时按字节数掐断,不能靠头
        .user_agent(crate::USER_AGENT)
        .build()
        .unwrap_or_default()
}

fn content_type_of(html: &str) -> &'static str {
    let lower = html.trim_start().to_ascii_lowercase();
    if lower.starts_with("{") || lower.starts_with("[") {
        "json"
    } else if lower.starts_with("<!doctype svg") || lower.starts_with("<svg") {
        "svg"
    } else if lower.starts_with("%pdf") {
        "pdf"
    } else if lower.starts_with("gif8") || lower.starts_with("riff") {
        // GIF 和 WEBP 的魔数是纯 ASCII,解码成 str 之后还在。
        // PNG、JPEG 的魔数含非法 UTF-8 字节,过一遍 `text()` 已经被
        // 替换字符污染了,认不出来——那就老实报 unknown,别报一个猜的
        "image"
    } else {
        "unknown"
    }
}

/// 是不是 HTML。**看的是有没有标签,不是看 Content-Type**——
/// 服务端标错类型的情况多得很,而正文抽取只认真标签
fn looks_like_html(html: &str) -> bool {
    let head = html.trim_start();
    let lower = head.to_ascii_lowercase();
    if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
        return true;
    }
    // 没有 doctype 的碎片:找第一个标签,得是文章会有的那几种
    for tag in ["<article", "<main", "<body", "<div", "<p ", "<h1", "<h2", "<meta"] {
        if lower.contains(tag) {
            return true;
        }
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;

    /// **不该抓的地址一律不碰。** 大部分剪藏是用户复制的一段话,
    /// 地址是编辑器里拖进来的、`file:///` 的、压根没链接的——
    /// 对着那些发一次网络请求,是必然失败的往返
    #[test]
    fn 非http地址不抓() {
        assert!(!looks_fetchable("file:///C:/a.html"));
        assert!(!looks_fetchable("javascript:void(0)"));
        assert!(!looks_fetchable("data:text/html,<h1>hi"));
        assert!(!looks_fetchable(""));
        assert!(!looks_fetchable("   "));
    }

    /// 太短的字符串即使以 https 开头也不是地址。多半是用户复制过来的
    /// 半截 `https://`
    #[test]
    fn 太短的地址不抓() {
        assert!(!looks_fetchable("https://"));
        assert!(!looks_fetchable("https://a"));
    }

    #[test]
    fn http地址该抓() {
        assert!(looks_fetchable("https://example.com/article"));
        assert!(looks_fetchable("http://example.com"));
        // 前后有空白也认
        assert!(looks_fetchable("  https://example.com/a  "));
    }

    /// **两道闸门都得真的插在读取的路上,而且响应头那道得在读之前。**
    ///
    /// 上面那些测试只证明了 `size_gate` 自己判得对。可它要是压根没被调用,
    /// 那些测试照样全绿——**这就是"两个函数各自都对、中间那段路没接上"**。
    /// 症状是 30MB 的页面照读不误,而所有断言都还是绿的
    #[test]
    fn 两道闸门都插在读取的路上() {
        let src = include_str!("fetch.rs");
        let start = src.find("pub fn fetch_page").expect("源码里没有 fetch_page");
        let tail = &src[start..];
        let end = tail.find("fn build_client").expect("fetch_page 后面找不到 build_client");
        let body = &tail[..end];

        let head = body.find("size_gate(len)?").expect("响应头那道闸门没插上");
        let read = body.find("resp.text()").expect("源码里找不到 resp.text()");
        let actual = body.find("size_gate(html.len())?").expect("读完那道闸门没插上");

        assert!(
            head < read,
            "响应头闸门得在读正文**之前**。放它后面就等于没放——30MB 已经读进内存了才去问大小"
        );
        assert!(
            read < actual,
            "读完那道闸门得在读完之后,不然量的不是实际拿到的字节数"
        );
    }

    /// **不是 HTML 的别送进 webview。** 一个 PDF 被塞进 DOMParser,
    /// 抽出来的是一堆乱码当正文——比老实说"抓不到"糟得多
    #[test]
    fn 不像html的不认() {
        assert!(!looks_like_html("{\"title\":\"x\"}"));
        assert!(!looks_like_html("%PDF-1.7"));
        assert!(!looks_like_html("just some plain text"));
    }

    /// 碎片页面没 doctype,但文章会有的标签够判了。
    /// **别因为没有 <!DOCTYPE> 就把半篇文章扔掉**——服务端返回的片段
    /// 常常只有 <body> 里的一截
    #[test]
    fn 碎片html也认() {
        assert!(looks_like_html("<!DOCTYPE html><html><body>hi</body></html>"));
        assert!(looks_like_html("<html lang='zh'><body></body></html>"));
        assert!(looks_like_html("<article><p>正文</p></article>"));
        assert!(looks_like_html("<div class='post'>内容</div>"));
    }

    /// 大小闸门是硬闸。**30MB 的页面塞进 webview 会让界面假死好几秒**
    #[test]
    fn 超过上限算太大() {
        const {
            assert!(
                MAX_HTML_BYTES >= 5 * 1024 * 1024,
                "闸门太小,正常文章会被误伤"
            );
            assert!(
                MAX_HTML_BYTES <= 50 * 1024 * 1024,
                "闸门太大,卡界面就说不清了"
            );
        }
    }

    /// **响应头那道闸门得真的会拦。** 它拦在读正文之前,
    /// 省掉的是把几十 MB 读进内存再扔掉的那几百毫秒——
    /// 之前这里根本没有这道闸,注释却在讲它
    #[test]
    fn 头部报太大了就拦() {
        let over = MAX_HTML_BYTES + 1;
        assert_eq!(size_gate(over).unwrap_err(), FetchError::TooLarge(over));
        assert!(size_gate(MAX_HTML_BYTES).is_ok(), "正好卡在上限的该放行");
        assert!(size_gate(0).is_ok());
    }

    /// **服务端把长度写错了,不能拿它去拦正常页面。** 有几个 CDN 会给
    /// 一个对不上的数,照它拦就是一篇正常文章打不开
    #[test]
    fn 认不出的头部长度就当没有() {
        use reqwest::header::{HeaderMap, HeaderValue, CONTENT_LENGTH};
        let mut h = HeaderMap::new();
        assert_eq!(declared_len(&h), None);
        h.insert(CONTENT_LENGTH, HeaderValue::from_static("12345"));
        assert_eq!(declared_len(&h), Some(12345));
        // 头是合法的,但内容不是一个数——这种也得退回去
        h.insert(CONTENT_LENGTH, HeaderValue::from_static("not-a-number"));
        assert_eq!(declared_len(&h), None, "解析不出来就该退回去实读那道");
    }

    /// 真去抓一个不存在的地址。**必须返回 Err 而不是 panic**——
    /// 抓不到是正常结局,不是崩溃理由
    #[test]
    fn 抓不通也不炸() {
        // 端口 1 上不会有服务在听。这条测的是"错误被处理了",
        // 不是"网络通了"——CI 环境里 DNS 可能就解析不了
        let r = fetch_page("http://127.0.0.1:1/nope");
        assert!(r.is_err(), "打不通就该是 Err");
        assert!(
            matches!(r.unwrap_err(), FetchError::Network(_) | FetchError::Timeout),
            "该归到网络/超时那一类"
        );
    }

    #[test]
    fn 非http地址直接报错不用联网() {
        assert_eq!(fetch_page("file:///a").unwrap_err(), FetchError::NotHttp("file:///a".into()));
    }
}
