//! 把文章里的图片下载到本地,让剪藏真的"存得住"。
//!
//! 剪藏的意义是"以后还能看"。图片还挂在原站上,这件事就不成立:
//! 站点关了就全裂,而且打开文章时图片服务器就知道你什么时候看的哪篇——
//! 和"剪藏全程不过任何服务器"这句话直接打架。
//!
//! 这里只放纯逻辑;网络部分由调用方通过 `localize` 的 `fetch` 闭包注入,
//! 这样扫描、改写、落盘这些都能在测试里拿假 fetch 跑,不碰网络。

use std::path::Path;

/// 一张图的上限。20MB 够一张长图截图,再大的多半是原图扫描件或者
/// 视频封面,塞进剪藏库里只会把用户磁盘塞满。
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// 一篇最多下多少张。防止一篇图集文章把磁盘和网络一起撑爆。
pub const MAX_IMAGES_PER_CLIP: usize = 200;

pub const ASSETS_DIR: &str = "assets";

/// 一处图片引用:地址是什么、在正文里的哪一段。
struct Ref<'a> {
    url: &'a str,
    /// 只包含地址本身那几段字节,不含 `](` / `src="` 这些壳
    start: usize,
    end: usize,
    /// 收尾引号。Markdown 的 `](url)` 没有引号,HTML 的有
    quote: Option<char>,
}

/// 扫出正文里所有图片引用的位置。
///
/// **扫描和改写必须走同一个函数。** 早先各写各的,结果扫描认得
/// `<img src=...>` 而改写只认 `](url)`,于是一段 `<img>` 的地址在
/// 「找得到要下载」和「下载完替换得掉」之间对不上号——那种不一致
/// 轻则图片白下一遍,重则把用户正文改坏。
fn scan_refs(markdown: &str) -> Vec<Ref<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    // **按字符走,不按字节。** 中文正文里一个 `标` 就是 3 个字节,按字节
    // 递增会停在字符中间,后面 `markdown[i..]` 直接 panic
    let next_boundary = |markdown: &str, from: usize| {
        let mut n = from + 1;
        while n < markdown.len() && !markdown.is_char_boundary(n) {
            n += 1;
        }
        n
    };
    while i < markdown.len() {
        // `![alt](url)` —— 前面那个 `!` 是图片和普通链接的分界
        if markdown[i..].starts_with("![") {
            if let Some(close) = markdown[i + 2..].find("](") {
                let start = i + 2 + close + 2;
                if let Some(len) = markdown[start..].find(')') {
                    let end = start + len;
                    let url = markdown[start..end].trim();
                    let lead = start
                        + (markdown[start..end].len() - markdown[start..end].trim_start().len());
                    out.push(Ref {
                        url: &markdown[lead..lead + url.len()],
                        start: lead,
                        end: lead + url.len(),
                        quote: None,
                    });
                    i = end;
                    continue;
                }
            }
            i = next_boundary(markdown, i + 1);
            continue;
        }
        // `<img ... src="url" ...>` —— 别的 HTML 标签不看,`<a href>` 里的
        // 地址是链接不是图,替掉它等于把用户的正文改坏
        if markdown[i..].starts_with("<img") {
            let tag_end = markdown[i..].find('>').map(|e| i + e);
            if let Some(tag_end) = tag_end {
                let tag = &markdown[i..tag_end];
                if let Some((value, quote)) = attribute(tag, "src") {
                    let start = i + (tag.find(&value).unwrap_or(0));
                    out.push(Ref {
                        url: &markdown[start..start + value.len()],
                        start,
                        end: start + value.len(),
                        quote: Some(quote),
                    });
                }
                i = tag_end + 1;
                continue;
            }
        }
        i = next_boundary(markdown, i);
    }
    out
}

/// 从 Markdown 里挑出**远程**图片地址,按出现顺序去重。
///
/// `data:` 内联图和已经指向本地的都不用管。
pub fn extract_image_urls(markdown: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in scan_refs(markdown) {
        if is_remote_http(r.url) && !out.iter().any(|u| u == r.url) {
            out.push(r.url.to_string());
        }
    }
    out
}

fn is_remote_http(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// 抠出 `name="value"` / `name='value'`,连引号一起返回。写这么点东西
/// 不用引正则——多一个正则依赖不值当。
fn attribute(tag: &str, name: &str) -> Option<(String, char)> {
    let bytes = tag.as_bytes();
    let mut from = 0;
    while let Some(at) = tag[from..].find(name) {
        let at = from + at;
        from = at + name.len();
        // 名字得是完整的一个属性,不能从 `data-src` 里蹭出一个 src
        let ok = at == 0
            || !(bytes[at - 1].is_ascii_alphanumeric()
                || bytes[at - 1] == b'-'
                || bytes[at - 1] == b':');
        if !ok {
            continue;
        }
        let rest = tag[from..].trim_start();
        let rest = rest.strip_prefix('=')?.trim_start();
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            continue;
        }
        let rest = &rest[1..];
        let end = rest.find(quote)?;
        return Some((rest[..end].to_string(), quote));
    }
    None
}

/// 图片在剪藏库里的相对位置。**必须用正斜杠**——Markdown 按 URL 语法
/// 解析路径,Windows 上写成 `assets\x\0.png` 的话 Obsidian 之类读不出来。
pub fn relative_path(clip_id: &str, index: usize, extension: &str) -> String {
    format!("{ASSETS_DIR}/{clip_id}/{index}.{extension}")
}

/// 从 Content-Type 推扩展名,推不出来就退到 URL 的后缀。都不是图片就返回空串。
///
/// 存成 `.bin` 也能读,但用户拿文件管理器翻剪藏目录时,一堆 `0.bin` 看不出
/// 是什么,还是按真实类型存。
pub fn extension_for(content_type: Option<&str>, url: &str) -> String {
    if let Some(ct) = content_type {
        let base = ct
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if base.starts_with("image/") {
            return match base.as_str() {
                "image/jpeg" | "image/jpg" => "jpg",
                "image/png" => "png",
                "image/gif" => "gif",
                "image/webp" => "webp",
                "image/avif" => "avif",
                "image/svg+xml" => "svg",
                "image/bmp" => "bmp",
                "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
                _ => "",
            }
            .to_string();
        }
        // 服务器明确说了不是图片(常见的是重定向回了 HTML 错误页),就别存
        if !base.is_empty() && base != "application/octet-stream" {
            return String::new();
        }
    }
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some(ext)
            if !ext.is_empty()
                && ext.len() <= 5
                && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            ext.to_ascii_lowercase()
        }
        _ => "bin".to_string(),
    }
}

/// 把成功下载的图换成相对路径,其余原样不动。
///
/// **只碰确实下成功的那几个,而且只碰图片位置上的那几段。** 下失败的保持
/// 原地址:宁可让它裂着,也好过把链接改成不存在的本地路径——那样用户连
/// "图原来在哪"都找不回来。正文里碰巧出现的同一个网址、指向图片的普通
/// 链接,一个字都不动。
pub fn rewrite_image_srcs(markdown: &str, map: &[(String, String)]) -> String {
    if map.is_empty() {
        return markdown.to_string();
    }
    let mut out = String::with_capacity(markdown.len());
    let mut cursor = 0;
    for r in scan_refs(markdown) {
        let Some((_, local)) = map.iter().find(|(remote, _)| remote == r.url) else {
            continue;
        };
        // 落在扫描区间之前的内容原样搬过去
        out.push_str(&markdown[cursor..r.start]);
        out.push_str(local);
        cursor = r.end;
        let _ = r.quote;
    }
    out.push_str(&markdown[cursor..]);
    out
}

/// 下载一篇剪藏里的所有远程图片,并把正文里的地址换成相对路径。
///
/// 返回 `(下载成功数, 失败数)`。**任何一张图失败都不算整体失败**——图片
/// 只是锦上添花,不该因为一张 403 就把整篇剪藏的流程搅黄。
///
/// `fetch` 用 reqwest 阻塞客户端:调用方跑在自己的后台线程里,不占 UI。
pub fn localize<F>(
    markdown: &str,
    clip_id: &str,
    assets_root: &Path,
    fetch: F,
) -> Result<(String, usize, usize), String>
where
    F: Fn(&str) -> Result<(Option<String>, Vec<u8>), String>,
{
    let urls = extract_image_urls(markdown);
    if urls.is_empty() {
        return Ok((markdown.to_string(), 0, 0));
    }
    let dir = assets_root.join(clip_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("建图片目录失败:{e}"))?;

    let mut done: Vec<(String, String)> = Vec::new();
    let mut failed = 0usize;
    // 下标只在成功的时候递增,文件名之间不会留空洞
    let mut index = 0usize;
    for url in urls.iter().take(MAX_IMAGES_PER_CLIP) {
        match fetch(url) {
            Ok((content_type, bytes)) => {
                if bytes.is_empty() || bytes.len() as u64 > MAX_IMAGE_BYTES {
                    failed += 1;
                    continue;
                }
                let ext = extension_for(content_type.as_deref(), url);
                if ext.is_empty() {
                    failed += 1;
                    continue;
                }
                let rel = relative_path(clip_id, index, &ext);
                let file = dir.join(format!("{index}.{ext}"));
                if std::fs::write(&file, &bytes).is_ok() {
                    done.push((url.clone(), rel));
                    index += 1;
                } else {
                    failed += 1;
                }
            }
            Err(_) => failed += 1,
        }
    }
    // 超出配额的也要算失败,不然调用方会以为全下齐了
    failed += urls.len().saturating_sub(MAX_IMAGES_PER_CLIP);

    // 一张都没下成就把空目录收了。空文件夹留在剪藏库里,用户翻目录时
    // 会以为"这篇本来就没图",其实只是下失败了
    if index == 0 {
        let _ = std::fs::remove_dir(&dir);
    }

    Ok((rewrite_image_srcs(markdown, &done), done.len(), failed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn 只认远程的图() {
        let md = "![a](https://cdn.example.com/1.png)\n\n<img src=\"http://x.com/2.jpg\">\n\n![b](assets/abc/0.png)\n\n![c](data:image/png;base64,AAA)\n";
        let urls = extract_image_urls(md);
        assert_eq!(
            urls,
            vec!["https://cdn.example.com/1.png", "http://x.com/2.jpg"]
        );
    }

    #[test]
    fn 同一张图出现两次只下一次() {
        let md = "![](https://a.com/1.png)\n\n![](https://a.com/1.png)\n";
        assert_eq!(extract_image_urls(md), vec!["https://a.com/1.png"]);
    }

    #[test]
    fn 没有图就别费事() {
        assert!(extract_image_urls("# 标题\n\n就一段文字。").is_empty());
    }

    #[test]
    fn 相对路径永远用正斜杠() {
        // Markdown 里的路径是按 URL 语法解析的,Windows 上写成反斜杠
        // 的话 Obsidian 之类根本认不出来
        assert_eq!(relative_path("m8x2", 3, "png"), "assets/m8x2/3.png");
    }

    #[test]
    fn 扩展名优先听服务器说的() {
        assert_eq!(extension_for(Some("image/webp"), "https://a.com/x"), "webp");
        assert_eq!(
            extension_for(Some("image/jpeg; charset=binary"), "https://a.com/x"),
            "jpg"
        );
        // 服务器不说就退到 URL 里的后缀
        assert_eq!(extension_for(None, "https://a.com/pic.GIF"), "gif");
        assert_eq!(extension_for(None, "https://a.com/pic"), "bin");
    }

    #[test]
    fn 不是图片就别存() {
        assert_eq!(extension_for(Some("text/html"), "https://a.com/x"), "");
        assert_eq!(
            extension_for(Some("application/pdf"), "https://a.com/x"),
            ""
        );
    }

    #[test]
    fn 把下好的图换成相对路径() {
        let md = "![封面](https://cdn.example.com/a.png)\n\n正文。\n\n<img src=\"https://cdn.example.com/b.jpg\" alt=\"图\">\n";
        let map = vec![
            (
                "https://cdn.example.com/a.png".to_string(),
                "assets/m1/0.png".to_string(),
            ),
            (
                "https://cdn.example.com/b.jpg".to_string(),
                "assets/m1/1.jpg".to_string(),
            ),
        ];
        let out = rewrite_image_srcs(md, map.as_slice());
        assert!(out.contains("![封面](assets/m1/0.png)"), "{out}");
        assert!(
            out.contains(r#"<img src="assets/m1/1.jpg" alt="图">"#),
            "{out}"
        );
        assert!(!out.contains("cdn.example.com"), "{out}");
        assert!(out.contains("正文。"), "{out}");
    }

    #[test]
    fn 没下成功的图保持原样() {
        // 下载失败的图必须还指着原站。宁可让它裂,也不能把链接改成
        // 一个不存在的本地路径——那样用户连"图原来在哪"都找不回来了
        let md = "![a](https://cdn.example.com/ok.png)\n\n![b](https://cdn.example.com/gone.png)\n";
        let map = vec![(
            "https://cdn.example.com/ok.png".to_string(),
            "assets/m1/0.png".to_string(),
        )];
        let out = rewrite_image_srcs(md, map.as_slice());
        assert!(out.contains("assets/m1/0.png"));
        assert!(out.contains("https://cdn.example.com/gone.png"), "{out}");
    }

    #[test]
    fn 改写不能动到别的内容() {
        let md =
            "# 标题\n\n[链接](https://a.com/1.png)\n\n正文里有 https://a.com/1.png 这个字样。\n";
        let map = vec![(
            "https://a.com/1.png".to_string(),
            "assets/m1/0.png".to_string(),
        )];
        let out = rewrite_image_srcs(md, map.as_slice());
        // 裸链接和正文里的字样都不是图片,一个字都不能动
        assert_eq!(out, md);
    }

    /// 真的起一个本地 HTTP 服务,而不是 mock。**代理、超时、Content-Type
    /// 协商这些坑只有在真 socket 上才现形**,mock 出来的 fetch 永远是对的。
    fn serve(routes: Vec<(&'static str, &'static str, &'static [u8])>) -> String {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑端口");
        let port = listener.local_addr().unwrap().port();
        // **不 join。** 这个循环是 accept 到天荒地老,join 等的是它退出,
        // 等不到。测试进程结束线程自然就没了,端口又是独立分配的,互不干扰
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                // 把请求头读到空行,不然客户端会等
                loop {
                    let mut h = String::new();
                    match reader.read_line(&mut h) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if h.trim().is_empty() => break,
                        Ok(_) => {}
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let hit = routes.iter().find(|(p, _, _)| *p == path);
                // 测试服务器,写不出去就直接断,不必假装成功
                if let Some((_, ct, body)) = hit {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream
                        .write_all(head.as_bytes())
                        .and_then(|_| stream.write_all(body));
                } else {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn reqwest_fetch(url: &str) -> Result<(Option<String>, Vec<u8>), String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| e.to_string())?;
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
    }

    #[test]
    fn 真下真存_图落到剪藏库里() {
        const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";
        let base = serve(vec![("/a.png", "image/png", PNG)]);
        let dir = TempDir::new().unwrap();
        let md = format!("# 标题\n\n![封面]({base}/a.png)\n\n正文。\n");

        let (_body, ok, failed) = localize(&md, "m1", dir.path(), reqwest_fetch).unwrap();

        assert_eq!(ok, 1, "一张都没下下来");
        assert_eq!(failed, 0);
        let saved = dir.path().join("m1").join("0.png");
        assert_eq!(
            std::fs::read(&saved).unwrap(),
            PNG,
            "落盘的内容得跟服务器给的一模一样"
        );
    }

    #[test]
    fn 下好了要把正文里的地址换成本地路径() {
        const PNG: &[u8] = b"x";
        let base = serve(vec![("/a.png", "image/png", PNG)]);
        let dir = TempDir::new().unwrap();
        let md = format!("![封面]({base}/a.png)\n");

        let (new_body, _, _) = localize(&md, "m1", dir.path(), reqwest_fetch).unwrap();
        assert!(new_body.contains("assets/m1/0.png"), "{new_body}");
    }

    #[test]
    fn 服务器返回的不是图片就别存() {
        // 真实世界里这个太常见了:图挂了,CDN 回一个 HTML 错误页,200
        let base = serve(vec![("/a.png", "text/html", b"<html>404</html>")]);
        let dir = TempDir::new().unwrap();
        let md = format!("![封面]({base}/a.png)\n");

        let (_body, ok, failed) = localize(&md, "m1", dir.path(), reqwest_fetch).unwrap();

        assert_eq!((ok, failed), (0, 1));
        assert!(!dir.path().join("m1").join("0.bin").exists());
        assert!(!dir.path().join("m1").join("0.html").exists());
    }

    #[test]
    fn 有一张_404_不影响别的图() {
        const PNG: &[u8] = b"x";
        let base = serve(vec![("/ok.png", "image/png", PNG)]);
        let dir = TempDir::new().unwrap();
        let md = format!("![a]({base}/missing.png)\n\n![b]({base}/ok.png)\n");

        let (_body, ok, failed) = localize(&md, "m1", dir.path(), reqwest_fetch).unwrap();

        assert_eq!((ok, failed), (1, 1), "一张成功一张失败,不能整体崩");
        assert!(dir.path().join("m1").join("0.png").exists());
    }

    #[test]
    fn 没有图就别建目录() {
        let dir = TempDir::new().unwrap();
        let (_body, ok, failed) = localize("# 就一段文字\n", "m1", dir.path(), |_| {
            panic!("没图就不该发起任何下载")
        })
        .unwrap();
        assert_eq!((ok, failed), (0, 0));
        assert!(!dir.path().join("m1").exists(), "没图就不该留下空目录");
    }
}
