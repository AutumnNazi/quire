//! 剪藏全文检索。
//!
//! ## 为什么没有 SQLite
//!
//! 路线图原本写的是 FTS5。改掉了,理由有两条:
//!
//! 1. **体量还不需要。** 实测 1000 篇、release 构建、全库都含查询词的
//!    最坏情况(预筛一条都挡不下),一次检索 258ms。真实查询里绝大多数文档
//!    会被预筛挡掉,实际远快于此。为了这个体量引入一个 C 依赖
//!    (SQLite 要么找系统库、要么编 bundled),不值当。
//! 2. **和项目的立场一致。** README 里写着"索引随时可以删"。真没有索引
//!    比"有个可删的索引"更彻底:检索慢了就慢,数据一个字节都不会受影响。
//!
//! 真到了扛不住的规模(几万篇、或者用户明确要求),再上 FTS5,那时把
//! [`search`] 换成查库就行,上层接口不用动。
//!
//! ## 为什么是二元组分词
//!
//! FTS5 自带的 trigram 分词器有硬下限:少于 3 个字符的查询匹配不到任何行。
//! 而中文里「苹果」「淘宝」「编程」全是两字,trigram 直接废掉一半场景。
//!
//! 二元组(把连续汉字切成相邻两字的组合)刚好卡在 2 字这个下限上:
//! 「苹果」切出一个词条 `苹果`,文档里的「苹果手机」切出 `苹果`/`果手`/`手机`,
//! 两边对得上。ASCII 单词整体保留并转小写,中英混排都能查。
//!
//! 代价是不支持拼音搜索——那是以后的事,不影响现在能用。

use std::collections::HashSet;

use serde::Serialize;

use crate::frontmatter::{self, Frontmatter};
use crate::vault::{ClipSummary, Vault, VaultError};

/// 检索命中的一条结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub summary: ClipSummary,
    /// 命中的正文片段,带上下文。
    pub snippet: String,
    /// 越大越靠前。
    pub score: i64,
}

/// 标题命中比正文命中值钱得多——用户搜一个词,多半就是在找那篇文章本身,
/// 而不是想读某个长文里提到它的那一段。
const TITLE_WEIGHT: i64 = 3;
const BODY_WEIGHT: i64 = 1;

impl SearchHit {
    pub fn filename(&self) -> &str {
        &self.summary.filename
    }
}

/// 搜整个剪藏库。`query` 为空时返回空列表——空查询不是"全部",是"没搜"。
pub fn search(vault: &Vault, query: &str, limit: usize) -> Result<Vec<SearchHit>, VaultError> {
    // 查询先转小写再切,词条就是小写的了
    let query_lower = query.to_lowercase();
    let tokens = tokenize(&query_lower);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    // 去重:同一个词出现两次不代表更相关
    let wanted: HashSet<&str> = tokens.iter().copied().collect();

    let clips_dir = vault.clips_dir();
    if !clips_dir.exists() {
        return Ok(Vec::new());
    }

    // 先只攒「摘要 + 正文 + 分数」,不急着切片段。片段是拿正文做子串查找加
    // 前后各撑 60 个字符,单条不算便宜;截断前 1000 条都切一遍、最后只留 200 条,
    // 是这个函数最大的一笔冤枉开销。切出来还得原样扔掉 800 条。
    let mut scored: Vec<(ClipSummary, String, String, i64)> = Vec::new();
    for entry in std::fs::read_dir(&clips_dir)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some((block, body)) = frontmatter::split(&text) else {
            continue;
        };
        let fm = Frontmatter::parse(block);
        if fm.id.is_empty() {
            continue;
        }

        // 正文和标题各转一次小写,之后预筛和分词共用这两份,不再重复分配。
        // 这是搜索的主要开销:每个文件原本要转两次小写。
        let body_lower = body.to_lowercase();
        let title_lower = fm.title.to_lowercase();

        // 便宜的预筛:词条里只要有一个在原文里压根没出现,这份文档就绝无可能
        // 命中。先用子串查一遍,把绝大多数文件挡在分词之前——不然每次按键都要
        // 把整个库重新切一遍词,输入框会明显发涩。
        if !wanted
            .iter()
            .any(|t| title_lower.contains(*t) || body_lower.contains(*t))
        {
            continue;
        }

        // 打分不做「整篇分词 + 建集合」:正文动辄几千字,切成二元组就是几千个
        // 切片,再全部塞进 HashSet 哈希一遍,最后只为回答「这几个词条在不在」。
        // 直接判定就够,见 has_token。
        let mut score = 0i64;
        for token in &wanted {
            if has_token(&title_lower, token) {
                score += TITLE_WEIGHT;
            }
            if has_token(&body_lower, token) {
                score += BODY_WEIGHT;
            }
        }
        if score == 0 {
            continue;
        }

        scored.push((
            crate::vault::summary_from(fm, filename.to_string()),
            body_lower,
            title_lower,
            score,
        ));
    }

    // 同分按 id 倒序,也就是新剪的在前——id 自带时间序,不用再读 clipped_at
    scored.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| b.0.id.cmp(&a.0.id)));
    scored.truncate(limit);

    Ok(scored
        .into_iter()
        .map(|(summary, body, title, score)| {
            let title_tokens: HashSet<&str> = tokenize(&title).into_iter().collect();
            let snippet_tokens: Vec<&str> = wanted
                .iter()
                .copied()
                .chain(title_tokens.iter().copied())
                .collect();
            SearchHit {
                summary,
                snippet: snippet(&body, snippet_tokens.into_iter()),
                score,
            }
        })
        .collect())
}

/// 判断 `text`(已转小写)里有没有这个词条。
///
/// 词条只有两种形态,判定方式也就两种:
///
/// - **二元组**:两个汉字。`contains` 就是精确判定——文本里出现这两个相邻
///   汉字时,它们之间没有别的字符,自然落在同一个连续汉字段里,一定会被
///   切成这个词条。反过来也成立。
/// - **单词**:一串非汉字的字母数字(英文、日文假名、俄文都算)。这时
///   `contains` 是不够的——搜 `rust` 不该命中 `rustacean`。得逐个找出出现
///   的位置,看左右两边是不是词边界。
///
/// 存在的意义是绕开「整篇分词 + 建 HashSet」:正文几千字切成二元组就是几千
/// 个切片,全哈希一遍只为回答几个词条在不在,不值当。
fn has_token(text: &str, token: &str) -> bool {
    // 汉字二元组:两个都是 CJK
    if token.chars().count() == 2 && token.chars().all(is_cjk) {
        return text.contains(token);
    }

    // 单词:要求左右都不是字母数字
    let mut from = 0;
    while let Some(i) = text[from..].find(token) {
        let at = from + i;
        let left_ok = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let right_at = at + token.len();
        let right_ok = text[right_at..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if left_ok && right_ok {
            return true;
        }
        // 接着找下一个,注意推进一个字符而不是一个字节——token 可能是多字节
        from = at + token.chars().next().map(char::len_utf8).unwrap_or(1);
    }
    false
}

/// 从正文里截一段带上下文的片段出来。
///
/// 找的是**原文里的位置**,不是词条拼回去——二元组是从原文切出来的,
/// 直接拿它去原文里找就能定位,不用维护"词条 → 字符下标"的映射。
fn snippet<'a>(body: &str, tokens: impl Iterator<Item = &'a str>) -> String {
    const WINDOW: usize = 60;

    // body 传进来时已经是小写版,词条也是小写的,直接找
    let Some(start) = tokens.filter_map(|t| body.find(t)).min() else {
        return first_chars(body, WINDOW);
    };

    // 往两边撑开,别从半个词开始
    let begin = body[..start]
        .char_indices()
        .rev()
        .take(WINDOW)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(start);
    let end = body[start..]
        .char_indices()
        .take(WINDOW)
        .last()
        .map(|(i, c)| start + i + c.len_utf8())
        .unwrap_or(body.len());

    let mut out = String::new();
    if begin > 0 {
        out.push('…');
    }
    out.push_str(body[begin..end].trim());
    if end < body.len() {
        out.push('…');
    }
    out
}

fn first_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// 切词条。ASCII 单词整体保留,汉字连续段切二元组,其余当分隔符。
///
/// **返回借用自入参的切片,不分配。** 二元组在原文里本来就是相邻的两个字,
/// 也就是一段连续的字节,直接切 `&str` 就行;ASCII 单词同理。只有调用方
/// 传进来的字符串活得够久,返回的切片才有效——搜索里传的是函数内的局部
/// 变量,正好匹配。
///
/// 入参应当**已经转好小写**:调用方会先给正文转一次小写,这里直接切,
/// 省掉每个文件一次全量 to_lowercase 分配。实测这一项就是搜索的主要开销。
pub fn tokenize(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    // 当前连续段的起点。ASCII 段和汉字段各记一个,遇到另一类或分隔符就收掉。
    let mut ascii_start: Option<usize> = None;
    let mut cjk_start: Option<usize> = None;

    for (i, ch) in text.char_indices() {
        if is_cjk(ch) {
            if let Some(s) = ascii_start.take() {
                tokens.push(&text[s..i]);
            }
            cjk_start.get_or_insert(i);
        } else if ch.is_alphanumeric() {
            if let Some(s) = cjk_start.take() {
                push_bigrams(&text[s..i], &mut tokens);
            }
            ascii_start.get_or_insert(i);
        } else {
            // 分隔符:两段都得当场收掉。漏了这一支,「苹果,手机」会被当成
            // 一整段切出「果,手」这种跨标点的二元组,查「手机」就搜不到了。
            // ASCII 段和汉字段不会同时存在(切换时会把另一段 take 掉),
            // 但都判一遍省得以后改逻辑踩坑。
            if let Some(s) = ascii_start.take() {
                tokens.push(&text[s..i]);
            }
            if let Some(s) = cjk_start.take() {
                push_bigrams(&text[s..i], &mut tokens);
            }
        }
    }

    let end = text.len();
    if let Some(s) = ascii_start {
        tokens.push(&text[s..end]);
    }
    if let Some(s) = cjk_start {
        push_bigrams(&text[s..end], &mut tokens);
    }
    tokens
}

/// 把一段连续汉字切成相邻二字的切片。
fn push_bigrams<'a>(run: &'a str, out: &mut Vec<&'a str>) {
    // 记的是上一个字的**起始**下标,当前字是它的搭档。记成结束下标的话
    // 会切出「果/手/机」这种丢掉首字的组合。
    let mut prev_start: Option<usize> = None;
    for (i, ch) in run.char_indices() {
        let end = i + ch.len_utf8();
        match prev_start {
            None => prev_start = Some(i),
            Some(p) => {
                out.push(&run[p..end]);
                prev_start = Some(i);
            }
        }
    }
}

/// CJK 统一表意文字(基本区 + 扩展 A)。扩展 B 以后和兼容区先不管——
/// 日常文章里几乎不会出现,漏掉不影响实际检索。
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::vault::ClipInput;
    use tempfile::TempDir;

    fn vault_with(clips: &[(&str, &str, &str)]) -> (TempDir, Vault) {
        let dir = TempDir::new().expect("建临时目录");
        let v = Vault::new(dir.path());
        for (url, title, body) in clips {
            v.save(&ClipInput {
                schema_version: 1,
                url: url.to_string(),
                title: title.to_string(),
                site_name: String::new(),
                author: None,
                excerpt: None,
                markdown: body.to_string(),
                published_at: None,
                image: None,
                favicon: None,
            })
            .expect("应保存成功");
        }
        (dir, v)
    }

    #[test]
    fn 两字中文查询能命中() {
        // 这是不用 trigram 的全部理由:trigram 少于三字匹配不到任何行,
        // 「苹果」这种查询会直接返回空
        let (_d, v) = vault_with(&[("https://a.com/1", "手机评测", "这台苹果手机很好用")]);
        let hits = search(&v, "苹果", 10).unwrap();
        assert_eq!(hits.len(), 1, "两字中文必须能搜到");
        assert!(hits[0].snippet.contains("苹果"));
    }

    #[test]
    fn 三字及以上的查询也能命中() {
        let (_d, v) = vault_with(&[("https://a.com/1", "深度解析", "我们来看看所有权模型")]);
        assert_eq!(search(&v, "所有权", 10).unwrap().len(), 1);
    }

    #[test]
    fn 英文查询大小写不敏感() {
        let (_d, v) = vault_with(&[("https://a.com/1", "Rust", "Learning Rust ownership")]);
        assert_eq!(search(&v, "rust", 10).unwrap().len(), 1);
        assert_eq!(search(&v, "RUST", 10).unwrap().len(), 1);
    }

    #[test]
    fn 中英混排的查询能拆成两半各自命中() {
        let (_d, v) = vault_with(&[("https://a.com/1", "笔记", "关于 Rust 所有权的笔记")]);
        // 词条是 rust / 所有 / 有权,三个都得在文档里
        assert_eq!(search(&v, "Rust 所有权", 10).unwrap().len(), 1);
    }

    #[test]
    fn 标题命中的排在正文命中之前() {
        let (_d, v) = vault_with(&[
            (
                "https://a.com/1",
                "完全不相干的标题",
                "这里提到了编程这个词",
            ),
            ("https://a.com/2", "编程", "这里讲的是别的东西"),
        ]);
        let hits = search(&v, "编程", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].summary.title, "编程", "标题命中该排前面");
    }

    #[test]
    fn 空查询返回空列表而不是全部() {
        // 空查询当"全部"的话,用户一进搜索框就会看到一堆无关结果
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "正文")]);
        assert!(search(&v, "", 10).unwrap().is_empty());
        assert!(search(&v, "   ", 10).unwrap().is_empty());
    }

    #[test]
    fn 搜不到就老实返回空() {
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "正文")]);
        assert!(search(&v, "量子力学", 10).unwrap().is_empty());
    }

    #[test]
    fn 限制条数真的生效() {
        let clips: Vec<(String, String, String)> = (0..5)
            .map(|i| {
                (
                    format!("https://a.com/{i}"),
                    "同一个标题".to_string(),
                    "同一个词".to_string(),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str, &str)> = clips
            .iter()
            .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
            .collect();
        let (_d, v) = vault_with(&borrowed);
        assert_eq!(search(&v, "同一个词", 2).unwrap().len(), 2);
    }

    #[test]
    fn 空剪藏库搜索不报错() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        assert!(search(&v, "任何词", 10).unwrap().is_empty());
    }

    #[test]
    fn 分词把汉字切成相邻二字() {
        let tokens = tokenize("苹果手机");
        assert!(tokens.contains(&"苹果"), "实际: {tokens:?}");
        assert!(tokens.contains(&"果手"), "实际: {tokens:?}");
        assert!(tokens.contains(&"手机"), "实际: {tokens:?}");
    }

    #[test]
    fn 单个汉字不成词() {
        // 查一个字必然命中一大堆,没有区分度,不如不查
        assert!(tokenize("我").is_empty());
    }

    #[test]
    fn 标点把汉字段切开() {
        // 「苹果，手机」是两截,不该被拼成「果手」这种跨标点的二元组
        assert_eq!(tokenize("苹果，手机。"), vec!["苹果", "手机"]);
    }

    #[test]
    fn 英文查询不命中更长的单词() {
        // 搜 rust 不该把 rustacean 也拽出来——用户搜的是 Rust 这门语言,
        // 不是碰巧含这四个字母的任何词
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "The rustacean is a crab")]);
        assert!(
            search(&v, "rust", 10).unwrap().is_empty(),
            "rust 是 rustacean 的子串,不该算命中"
        );
    }

    #[test]
    fn 英文查询作为独立单词能命中() {
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "Learning Rust ownership")]);
        assert_eq!(search(&v, "rust", 10).unwrap().len(), 1);
    }

    #[test]
    fn 词条判定和分词结果一致() {
        // has_token 是为了绕开分词而写的等价判定,不是另立一套规则。
        // 拿一批样本对着 tokenize 建出来的集合逐个核对,免得两边悄悄跑偏。
        let samples = [
            "苹果手机评测",
            "关于 Rust 所有权的笔记",
            "The rustacean is a crab, not Rust.",
            "用 2024 年的 Go 写点东西",
            "Mixed 中英文 content_here 排版",
        ];
        for sample in samples {
            let lower = sample.to_lowercase();
            let set: HashSet<&str> = tokenize(&lower).into_iter().collect();
            for token in &set {
                assert!(
                    has_token(&lower, token),
                    "分词出的词条「{token}」在「{sample}」里判定不出来"
                );
            }
            // 反向也查一遍:不该命中的东西别混进来
            for bogus in ["苹果", "rust", "所有权", "2024", "内容"] {
                let want = set.contains(bogus);
                assert_eq!(
                    has_token(&lower, bogus),
                    want,
                    "「{sample}」里「{bogus}」判定与分词不一致"
                );
            }
        }
    }

    #[test]
    fn 片段里能看到上下文() {
        let long = "前面很长的一段话".repeat(20);
        let body = format!("{long}关键在这里后面也很长{}", "尾巴".repeat(20));
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", &body)]);
        let hits = search(&v, "关键", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("关键"));
        assert!(
            hits[0].snippet.chars().count() < 200,
            "片段不该把整篇塞进来"
        );
    }
}

/// 造 N 篇剪藏。放在 tests 模块外,好让性能测试也能用。
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use crate::vault::ClipInput;
    use tempfile::TempDir;

    pub fn many_clips(n: usize) -> (TempDir, Vault) {
        let dir = TempDir::new().expect("建临时目录");
        let v = Vault::new(dir.path());
        for i in 0..n {
            let body = format!(
                "第 {} 篇的正文。{} 所有权是 Rust 的核心概念,讲了 {} 遍。{}",
                i,
                "铺垫内容。".repeat(40),
                i % 7 + 1,
                "无关的尾巴。".repeat(20)
            );
            v.save(&ClipInput {
                schema_version: 1,
                url: format!("https://example.com/{i}"),
                title: format!("第{i}篇文章"),
                site_name: String::new(),
                author: None,
                excerpt: None,
                markdown: body,
                published_at: None,
                image: None,
                favicon: None,
            })
            .expect("应保存成功");
        }
        (dir, v)
    }
}

#[cfg(test)]
mod perf {
    use super::tests_support::*;
    use super::*;

    /// 量一下真实体量下的耗时。注释里写了具体数字,那就得让它可核对——
    /// 免得哪天有人往搜索里塞个 O(n²),只有等到用户剪藏上千篇才发现卡。
    ///
    /// 造的数据里**每篇都含「所有权」**,预筛一条都挡不下,是最坏情况;
    /// 真实查询会明显更快。数字是 release 构建下的实测值。
    #[test]
    fn 千篇规模下搜得动() {
        let (_d, v) = many_clips(1000);
        let query = "所有权";
        let start = std::time::Instant::now();
        let hits = search(&v, query, 200).unwrap();
        let elapsed = start.elapsed();
        println!(
            "1000 篇(全部命中,最坏情况),搜「{query}」耗时 {elapsed:?},命中 {} 条",
            hits.len()
        );
        assert!(!hits.is_empty(), "造的数据里应该有命中");
        // 门限放得宽,只为挡住数量级的劣化;debug 构建比 release 慢不少
        assert!(elapsed.as_secs() < 5, "搜 1000 篇用了 {elapsed:?},太慢了");
    }
}
