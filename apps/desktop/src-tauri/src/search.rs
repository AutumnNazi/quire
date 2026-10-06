//! 剪藏全文检索。
//!
//! ## 两条路,结果必须一样
//!
//! 打分、切片段、排序的逻辑只有一份,在 [`search_dir`] 里。
//! [`search_indexed`] 是它前面加的一段**候选预筛**:先用 SQLite FTS5
//! 从索引里挑出"可能相关"的那几篇,再拿原文走同一套打分。
//!
//! **索引只负责挑候选,不负责决定结果。** 所以两条路必须给出逐条相同的
//! 文件名、顺序、分数、片段——`索引和扫描给出一样的结果` 那条测试盯着
//! 这个,是这个模块全部的安全网。分叉的后果是用户搜"苹果"时好时坏,
//! 而界面上没有任何线索能解释为什么
//!
//! ## 为什么现在上 FTS5
//!
//! 一开始是没有的。当时实测 1000 篇、release 构建、最坏情况(全库都含
//! 查询词,预筛一条都挡不下)一次检索 258ms,判断"这个体量不值当引入
//! C 依赖"。收藏和标签铺开之后单库规模上去了,而剪藏这个用法恰恰是
//! "越攒越多、搜得越来越频繁",扫一遍的耗时又随篇数线性涨
//!
//! 加进来的代价被摁在三件事上:
//!
//! 1. **索引是纯缓存。** 建不出来、坏了、schema 变了,一律退回逐文件扫描,
//!    用户只会觉得慢一点。把索引目录整个删掉,搜索结果一个字都不变
//! 2. **索引不进剪藏库。** 剪藏库多半开着同步盘,而 SQLite 文件放进同步
//!    目录是冲突重灾区——两边各改一份,同步下来就打架。索引待在应用
//!    自己的配置目录里
//! 3. **`.md` 永远是唯一真相。** 索引里不存任何别处没有的东西
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
//! 代价有两个,写在这儿免得哪天当成 bug 查:
//!
//! - **单个汉字搜不到。** 一字切不出二元组。扫描那条路(早于索引就在了)
//!   同样搜不到,所以这是既有边界,不是索引引入的偏差
//! - **不支持拼音。** 主动的决定,不做

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::frontmatter::{self, Frontmatter};
use crate::vault::{ClipSummary, UnreadableFile, Vault, VaultError};

/// 检索命中的一条结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub summary: ClipSummary,
    /// 命中的正文片段,带上下文。
    pub snippet: String,
    /// **`snippet` 里哪几段是命中的**,`[起, 止]` 是**字符**下标不是字节——
    /// 拿字节下标去切中文会在半个字上劈开,界面上就是一个乱码方块
    ///
    /// 单独一栏而不是往 `snippet` 里插 `<mark>`:正文里可能有真 HTML,
    /// 插进去之后这份片段既不能当纯文本用,也没法直接进导出文件
    #[serde(default)]
    pub marks: Vec<[u32; 2]>,
    /// 标题里命中的词,界面上可以把标题也标出来。`snippet` 之外的第二处
    #[serde(default)]
    pub title_marks: Vec<[u32; 2]>,
    /// **这条是"差一个字"找出来的,不是原样命中的。**
    ///
    /// 界面上必须标出来,而且**排在标准结果后面**。把两种混在一起不给区分,
    /// 用户点进去发现不是那篇,连着错两三次他就不搜了——那比一开始就搜不到更糟
    #[serde(default)]
    pub fuzzy: bool,
    /// 越大越靠前。
    pub score: i64,
}

/// 标题命中比正文命中值钱得多——用户搜一个词,多半就是在找那篇文章本身,
/// 而不是想读某个长文里提到它的那一段。
const TITLE_WEIGHT: i64 = 3;
const BODY_WEIGHT: i64 = 1;
/// 标签比正文重,比标题轻。用户主动打的标签是明确的意图,
/// 正文里碰巧出现同一个词可能只是顺带一提。
const TAG_WEIGHT: i64 = 2;
/// 批注和标签同权重。用户亲手写下的那句话,是关于这篇剪藏最明确的一句
/// 描述——比标题还明确,只是标题通常更短。
const NOTE_WEIGHT: i64 = 2;

impl SearchHit {
    pub fn filename(&self) -> &str {
        &self.summary.filename
    }
}

/// 一次搜索的结果。**跳过的文件必须一起带回来**。
///
/// 原来是三个 `continue`,读不出、没 frontmatter、没有 id 的文件全都悄悄
/// 没了。列表视图那边已经用红条告诉用户"有 3 篇读不出",一搜那 3 篇就没了,
/// 还不吭声——他明明记得那篇文章里有这个词,搜出来是空的,只会以为是自己记错了
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    /// 跳过了的 `.md`,连原因一起。字段和 `ScanResult::unreadable` 同名同义,
    /// 免得界面上一处叫 unreadable 一处叫 skipped,对着抄的时候抄错
    pub skipped: Vec<UnreadableFile>,
}

/// 搜整个剪藏库。`query` 为空时返回空列表——空查询不是"全部",是"没搜"。
pub fn search(
    vault: &Vault,
    query: &str,
    limit: usize,
    scope: Option<Scope>,
) -> Result<SearchOutcome, VaultError> {
    search_dir(&vault.clips_dir(), query, limit, scope)
}

/// 搜回收站。界面上回收站是一个独立视图,搜索框跟着它走——
/// 搜的却是**库里**的东西,用户会以为"我明明删了它怎么还搜得到"。
pub fn search_trash(
    vault: &Vault,
    query: &str,
    limit: usize,
    scope: Option<Scope>,
) -> Result<SearchOutcome, VaultError> {
    search_dir(&vault.trash_dir(), query, limit, scope)
}

/// 限定搜索范围。**决定候选从哪些字段来**
///
/// 权重是共用的(标题 3、正文 1、标签 2、批注 2),**但分数总和必然会变**:
/// 限定标题之后标签那两分就不算了。换个范围看到不同的分数,这是对的,
/// 不是 bug——两个范围下的分数本来就不是同一个量,拿来互相比没有意义
///
/// 真正要守住的不变量只有一条:**缩小范围只会少结果,不会多结果**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// 标题、正文、标签、批注全搜。**默认值**,行为和加这个参数之前一样
    #[default]
    Any,
    Title,
    Body,
    Tag,
    Note,
}

impl Scope {
    fn includes_title(self) -> bool {
        matches!(self, Scope::Any | Scope::Title)
    }

    fn includes_body(self) -> bool {
        matches!(self, Scope::Any | Scope::Body)
    }

    fn includes_tag(self) -> bool {
        matches!(self, Scope::Any | Scope::Tag)
    }

    fn includes_note(self) -> bool {
        matches!(self, Scope::Any | Scope::Note)
    }
}

/// 这次搜索实际走的是哪条路。
///
/// 存在的理由很实际:「索引和扫描给出一样的结果」是整个 FTS5 部分的安全网,
/// 而**索引那条路如果每次都悄悄退回扫描,这个安全网就是空的**——两条路
/// 本来就一模一样,测了等于没测。测试要能看见它到底走没走索引,
/// 这条路"平时真在跑"才有证据
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// 用了 SQLite 索引
    Index,
    /// 退回逐文件扫描了。**这不是错误**——索引是加速器,
    /// 建不出来或者对不齐,搜索就慢一点,不该坏掉
    Scan,
}

/// 走索引的那条路。
///
/// **索引坏了就退回扫描,不是报错。** 索引是加速器,不是必需品——
/// 它建不出来、文件被锁、schema 变了、用户手动删了,这些都只该让搜索
/// 慢一点,不该让它坏掉。剪藏的真相在 `.md` 里,索引一个字节都不是
///
/// 同步策略:先按文件名对一遍索引里有什么、磁盘上有什么,缺的补、多的删、
/// 变了的重索引。**不是每次全清重建**——上千篇重建一次几秒,
/// 而用户只是打了一个字
pub fn search_indexed(
    idx: &mut crate::index::Index,
    dir: &Path,
    query: &str,
    limit: usize,
    scope: Option<Scope>,
) -> (SearchOutcome, Route) {
    let scope = scope.unwrap_or_default();
    let query_lower = query.to_lowercase();
    let tokens: Vec<String> = tokenize(&query_lower)
        .into_iter()
        .map(str::to_string)
        .collect();
    if tokens.is_empty() {
        return (SearchOutcome::default(), Route::Scan);
    }
    let wanted: HashSet<&str> = tokenize(&query_lower).into_iter().collect();

    if sync_index(idx, dir) {
        return (search_dir(dir, query, limit, Some(scope)).unwrap_or_default(), Route::Scan);
    }

    let Ok(candidates) = idx.search(&tokens, scope) else {
        return (search_dir(dir, query, limit, Some(scope)).unwrap_or_default(), Route::Scan);
    };

    // 候选直接从索引来,**不再读磁盘**。打分用的就是存进去时那份原文
    let mut scored: Vec<(ClipSummary, String, String, i64)> = Vec::new();
    for c in candidates {
        let title_lower = c.title.to_lowercase();
        let body_lower = c.body.to_lowercase();
        let tags_lower = c.tags.to_lowercase();
        let note_lower = c.note.to_lowercase();
        let mut score = 0i64;
        for token in &wanted {
            if scope.includes_title() && has_token(&title_lower, token) {
                score += TITLE_WEIGHT;
            }
            if scope.includes_body() && has_token(&body_lower, token) {
                score += BODY_WEIGHT;
            }
            if scope.includes_tag() && has_token(&tags_lower, token) {
                score += TAG_WEIGHT;
            }
            if scope.includes_note() && has_token(&note_lower, token) {
                score += NOTE_WEIGHT;
            }
        }
        if score == 0 {
            continue;
        }
        // 摘要用索引里存的那份 frontmatter 重建,和走扫描那条路同一个
        // `Frontmatter::parse`——自己拆自己拼的话,两边总会各漏一个字段
        let fm = Frontmatter::parse(&c.fm);
        scored.push((
            crate::vault::summary_from(fm, c.id),
            body_lower,
            title_lower,
            score,
        ));
    }

    // **跳过的文件照样要报。** 索引里没有的(没 frontmatter、没有 id、
    // 读不出的)正是走扫描那条路会报出来的那批,不能因为走了索引就闭嘴
    let skipped = collect_skipped(dir);
    (
        SearchOutcome {
            hits: finish(scored, &wanted, limit),
            skipped,
        },
        Route::Index,
    )
}

/// 把索引和磁盘对齐。**返回 true 表示对不齐、该走扫描**。
///
/// 对不齐的判据是"数量差得离谱"——正常情况下只改了一篇两篇,一篇篇补
/// 就够了;差一大截(换了剪藏库、索引文件是旧的)整个重建比一篇篇对快得多
fn sync_index(idx: &mut crate::index::Index, dir: &Path) -> bool {
    if !dir.exists() {
        return true;
    }
    let mut on_disk: Vec<(String, u64, u64)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else { continue };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() * 1000 + d.subsec_millis() as u64)
            .unwrap_or(0);
        on_disk.push((name.to_string(), mtime, meta.len()));
    }
    if on_disk.is_empty() {
        // 库里一篇都没有,索引也该清空。不清的话用户删光了所有剪藏、
        // 搜索框里还留着刚才那个词,点进去报的是"已删除的剪藏还在索引里"
        // ——而界面上哪儿都没有它,用户完全无从判断这是缓存出了错
        let was_empty = idx.is_empty();
        if !was_empty && idx.clear().is_err() {
            return true;
        }
        return !was_empty;
    }
    // 差一大截就整个重建
    let gap = on_disk.len().abs_diff(idx.len());
    if gap > on_disk.len() / 2 + 1 && idx.clear().is_err() {
        return true;
    }
    // **磁盘上没了的,索引里也得没有。** 这一步原来压根没有,
    // 于是删掉的剪藏照样搜得到,点进去报"剪藏不存在"——
    // 用户在列表里明明已经看不到它了
    let keep: HashSet<String> = on_disk.iter().map(|(n, _, _)| n.clone()).collect();
    if idx.prune(&keep).is_err() {
        return true;
    }
    for (name, mtime, size) in &on_disk {
        if idx.stamp(name) == Some((*mtime, *size)) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else {
            continue;
        };
        let Some((block, body)) = frontmatter::split(&text) else {
            let _ = idx.remove(name);
            continue;
        };
        let fm = Frontmatter::parse(block);
        if fm.id.is_empty() {
            let _ = idx.remove(name);
            continue;
        }
        if idx.upsert(name, block, body, *mtime, *size).is_err() {
            return true;
        }
    }
    false
}

/// 扫一遍目录,把解析不了的挑出来。**和 `search_dir` 里的判据一模一样**——
/// 两条路报出来的清单必须一致,否则用户会看到"有时候列表有红条有时候没有"
fn collect_skipped(dir: &Path) -> Vec<crate::vault::UnreadableFile> {
    let mut skipped = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return skipped;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let filename = name.to_string();
        let Ok(text) = std::fs::read_to_string(&path) else {
            skipped.push(crate::vault::UnreadableFile {
                filename,
                reason: "读不出这个文件".to_string(),
            });
            continue;
        };
        let Some((block, _)) = frontmatter::split(&text) else {
            skipped.push(crate::vault::UnreadableFile {
                filename,
                reason: "没有 Quire 的 frontmatter,搜不了".to_string(),
            });
            continue;
        };
        if Frontmatter::parse(block).id.is_empty() {
            skipped.push(crate::vault::UnreadableFile {
                filename,
                reason: "frontmatter 里没有 id,不是 Quire 存的剪藏".to_string(),
            });
        }
    }
    skipped.sort_by(|a, b| a.filename.cmp(&b.filename));
    skipped
}

/// 逐文件扫的那条路,原样保留。
///
/// **索引那条路(`search_dir_indexed`)必须和它给出逐条相同的结果。**
/// 一旦两条路分叉,用户就会遇到"有时候搜得到有时候搜不到"——
/// 那比慢可怕得多,而且极难查:两条路都"看着对",只是结果集差一点。
/// `索引和扫描给出一样的结果` 那条测试就是这条约束的执行
fn search_dir(
    dir: &Path,
    query: &str,
    limit: usize,
    scope: Option<Scope>,
) -> Result<SearchOutcome, VaultError> {
    let scope = scope.unwrap_or_default();
    // 查询先转小写再切,词条就是小写的了
    let query_lower = query.to_lowercase();
    let tokens = tokenize(&query_lower);
    if tokens.is_empty() {
        return Ok(SearchOutcome::default());
    }
    // 去重:同一个词出现两次不代表更相关
    let wanted: HashSet<&str> = tokens.iter().copied().collect();

    if !dir.exists() {
        return Ok(SearchOutcome::default());
    }

    // 先只攒「摘要 + 正文 + 分数」,不急着切片段。片段是拿正文做子串查找加
    // 前后各撑 60 个字符,单条不算便宜;截断前 1000 条都切一遍、最后只留 200 条,
    // 是这个函数最大的一笔冤枉开销。切出来还得原样扔掉 800 条。
    let mut scored: Vec<(ClipSummary, String, String, i64)> = Vec::new();
    // 跳过的记下来,最后跟结果一起带回去。理由要说人话:用户要判断的是
    // "我要不要去把那个文件改一下",光给一句 io 错误他看不懂
    let mut skipped: Vec<UnreadableFile> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let filename = filename.to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                skipped.push(UnreadableFile {
                    filename,
                    reason: format!("读不出这个文件:{e}"),
                });
                continue;
            }
        };
        let Some((block, body)) = frontmatter::split(&text) else {
            skipped.push(UnreadableFile {
                filename,
                reason: "没有 Quire 的 frontmatter,搜不了".to_string(),
            });
            continue;
        };
        let fm = Frontmatter::parse(block);
        if fm.id.is_empty() {
            // 不是"坏文件",是手工放的笔记:Quire 自己写的每一篇都有 id。
            // 说得跟前面两种分开,不然用户会去改一个根本不用改的文件
            skipped.push(UnreadableFile {
                filename,
                reason: "frontmatter 里没有 id,不是 Quire 存的剪藏".to_string(),
            });
            continue;
        }

        // 正文和标题各转一次小写,之后预筛和分词共用这两份,不再重复分配。
        // 这是搜索的主要开销:每个文件原本要转两次小写。
        let body_lower = body.to_lowercase();
        let title_lower = fm.title.to_lowercase();
        // 标签拼成一份检索文本,和标题正文一样转小写后一起判。
        // 没有标签的剪藏绝大多数,这条会是空串,`has_token` 对空串直接否
        let tags_lower = if fm.tags.is_empty() {
            String::new()
        } else {
            fm.tags.join(" ").to_lowercase()
        };
        // 批注同理。绝大多数剪藏没写过批注,这里也是空串,
        // `has_token` 对空串直接否,不白干活
        let note_lower = fm.note.to_lowercase();

        // 便宜的预筛:词条里只要有一个在原文里压根没出现,这份文档就绝无可能
        // 命中。先用子串查一遍,把绝大多数文件挡在分词之前——不然每次按键都要
        // 把整个库重新切一遍词,输入框会明显发涩。
        if !wanted.iter().any(|t| {
            (scope.includes_title() && title_lower.contains(*t))
                || (scope.includes_body() && body_lower.contains(*t))
                || (scope.includes_tag() && tags_lower.contains(*t))
                || (scope.includes_note() && note_lower.contains(*t))
        }) {
            continue;
        }

        // 打分不做「整篇分词 + 建集合」:正文动辄几千字,切成二元组就是几千个
        // 切片,再全部塞进 HashSet 哈希一遍,最后只为回答「这几个词条在不在」。
        // 直接判定就够,见 has_token。
        let mut score = 0i64;
        for token in &wanted {
            if scope.includes_title() && has_token(&title_lower, token) {
                score += TITLE_WEIGHT;
            }
            if scope.includes_body() && has_token(&body_lower, token) {
                score += BODY_WEIGHT;
            }
            if scope.includes_tag() && has_token(&tags_lower, token) {
                score += TAG_WEIGHT;
            }
            if scope.includes_note() && has_token(&note_lower, token) {
                score += NOTE_WEIGHT;
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

    // 按文件名排,报错顺序才稳:用户第二次搜同一个词,看到的清单得和第一次一样
    skipped.sort_by(|a, b| a.filename.cmp(&b.filename));

    Ok(SearchOutcome {
        hits: finish(scored, &wanted, limit),
        skipped,
    })
}

/// 编辑距离 ≤ 1。**只认一次编辑**:插入、删除、替换,任选一种。
///
/// **两次以上不算。** 不是算不动,是**算了反而有害**:差两个字的两个词
/// 多半根本不是同一个词,把它们凑到一起给用户,他会以为 Quire 在胡说。
/// 差一个字才是"手滑"最可能的样子
fn within_one_edit(x: &[char], y: &[char]) -> bool {
    match x.len().abs_diff(y.len()) {
        // 一样长:至多一处不一样
        0 => x.iter().zip(y).filter(|(a, b)| a != b).count() <= 1,
        // 差一格:长的跳过一个字符之后必须完全一样
        1 => {
            let (long, short) = if x.len() > y.len() { (x, y) } else { (y, x) };
            let (mut i, mut j) = (0usize, 0usize);
            let mut skipped = false;
            while i < long.len() && j < short.len() {
                if long[i] == short[j] {
                    i += 1;
                    j += 1;
                    continue;
                }
                // 已经有一次不对了,后面还得再有就是两次编辑
                if skipped {
                    return false;
                }
                skipped = true;
                i += 1;
            }
            true
        }
        _ => false,
    }
}

/// 在 `haystack` 里找一段**长度等于 `needle`** 的窗口,和 `needle` 只差一次编辑。
///
/// 返回那个窗口的**字符**下标(不是字节)。找不到是 `None`。
///
/// **不做编辑距离的通用实现,是一个字一个字挪着比。** 通用 DP 要开一张
/// 矩阵、每个窗口重算一遍;这里只需要"差不超过一次",两种长度关系各有一种
/// 直接的判法,而比的是**定长的相邻窗口**——错字的形态就长这样
pub fn loose_window(haystack: &str, needle: &str) -> Option<usize> {
    let n: Vec<char> = needle.chars().collect();
    // **单字不参与。** 单字词条跟任何一个不相干的字距离都 ≤ 1,参与进来的
    // 后果是"搜一个字,半个库都算近似命中"
    if n.len() < 2 {
        return None;
    }
    let h: Vec<char> = haystack.chars().collect();
    if h.len() < n.len() {
        return None;
    }
    for start in 0..=(h.len() - n.len()) {
        if within_one_edit(&h[start..start + n.len()], &n) {
            return Some(start);
        }
    }
    None
}

/// 宽松搜索。**只在标准搜索一条都没命中时才走**,而且每一条都标着
/// `fuzzy`,界面上必须标出来。
///
/// 把"大概是这个"和"就是这个"混在一张列表里不给区分,用户点进去发现
/// 不是那篇,连着错两三次他就不搜了——那比一开始就搜不到更糟
pub fn search_loose(
    dir: &Path,
    query: &str,
    limit: usize,
    scope: Option<Scope>,
) -> Vec<SearchHit> {
    let scope = scope.unwrap_or_default();
    let query_lower = query.to_lowercase();
    let tokens: Vec<String> = tokenize(&query_lower)
        .into_iter()
        .filter(|t| t.chars().count() >= 2)
        .map(str::to_string)
        .collect();
    // 长到没边界的查询不去扫。**省的不只是时间**:一整段话进来,几乎每一篇
    // 都会有一处"差一个字",那不叫近似命中,那叫全库
    if tokens.is_empty() || query_lower.chars().count() > 32 {
        return Vec::new();
    }
    if !dir.exists() {
        return Vec::new();
    }

    let mut scored: Vec<(i64, ClipSummary, String, String)> = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten() {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some((block, body)) = frontmatter::split(&text) else {
            continue;
        };
        let fm = Frontmatter::parse(block);
        // 和标准搜索一样跳过手工放的笔记。宽松搜索是**补救**,不是另开一扇门:
        // 这里要是把没 id 的笔记也放进来,标准搜索和容错搜索看到的就不是同一个库
        if fm.id.is_empty() {
            continue;
        }

        let body_lower = body.to_lowercase();
        let title_lower = fm.title.to_lowercase();
        let tags_lower = fm.tags.join(" ").to_lowercase();
        let note_lower = fm.note.to_lowercase();

        // 命中的词条数。**不是"命中一篇"就一分**——用户搜三个词,
        // 一篇中了三个比一篇只中一个更可能是他要找的那篇
        let mut matched = 0usize;
        let mut score = 0i64;
        for token in &tokens {
            let mut hit = false;
            for (field, text, weight) in [
                (scope.includes_title(), &title_lower, TITLE_WEIGHT),
                (scope.includes_body(), &body_lower, BODY_WEIGHT),
                (scope.includes_tag(), &tags_lower, TAG_WEIGHT),
                (scope.includes_note(), &note_lower, NOTE_WEIGHT),
            ] {
                if field && loose_window(text, token).is_some() {
                    hit = true;
                    score += weight;
                }
            }
            if hit {
                matched += 1;
            }
        }
        // **至少要有一半的词条容错命中。**
        //
        // 只要"命中一个"的话,长查询几乎**每一篇**都能凑上一个:「扫地机器人」
        // 切出「扫地 / 地机 / 机器 / 器人」,而正文「换了新手机」里有个
        // 「手机」——「地机」和「手机」只差一个字。那不叫近似命中,那叫全库,
        // 而用户看到一堆不相干的东西之后就不信这个功能了
        if matched * 2 < tokens.len() {
            continue;
        }
        score += matched as i64 * 100;

        scored.push((
            score,
            crate::vault::summary_from(fm, name.to_string()),
            body_lower,
            title_lower,
        ));
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));
    scored.truncate(limit);

    scored
        .into_iter()
        .map(|(score, summary, body, _title)| SearchHit {
            // 近似命中**一律不高亮**。用户打错的是哪个字,我们并不知道:
            // 标出"手机"而他搜的是"手记",等于告诉他 Quire 认定他没打错字——
            // 那他更不会相信这批结果了
            marks: Vec::new(),
            title_marks: Vec::new(),
            snippet: head_snippet(&body),
            summary,
            score,
            fuzzy: true,
        })
        .collect()
}

/// 正文开头那一段。近似命中没有可标的位置,就别假装有,
/// 老实给开头——用户点进去看的是全文,片段只是给他一个方向
fn head_snippet(body_lower: &str) -> String {
    const SNIPPET_CHARS: usize = 140;
    let chars: Vec<char> = body_lower.chars().collect();
    let mut end = SNIPPET_CHARS.min(chars.len());
    // 截在词中间不像话。中文没有空格可切,退而求其次截在标点上,
    // 没有标点就硬截——**只是个方向,不是摘要**
    if let Some(cut) = chars[..end].iter().rposition(|c| "。！？；\n".contains(*c)) {
        end = cut + 1;
    }
    chars[..end].iter().collect()
}

/// 排序 + 截断 + 切片段。**索引那条路和扫描那条路共用这一份。**
///
/// 索引只负责"挑候选",挑完之后的三步全都走原来那套逻辑。
/// 共用一份的意义不是省代码,是**让两条路不可能分叉**:两个字段的权重、
/// 片段的取法、排序的稳定位都只有一处,改的时候没有"改了一处忘了另一处"
/// 的机会。一旦分叉,用户遇到的是"有时候搜得到有时候搜不到"——
/// 那比慢可怕得多,而且极难查
fn finish(
    mut scored: Vec<(ClipSummary, String, String, i64)>,
    wanted: &HashSet<&str>,
    limit: usize,
) -> Vec<SearchHit> {
    // 同分按 id 倒序,也就是新剪的在前——id 自带时间序,不用再读 clipped_at
    scored.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| b.0.id.cmp(&a.0.id)));
    scored.truncate(limit);

    scored
        .into_iter()
        .map(|(summary, body, title, score)| {
            let title_tokens: HashSet<&str> = tokenize(&title).into_iter().collect();
            let snippet_tokens: Vec<&str> = wanted
                .iter()
                .copied()
                .chain(title_tokens.iter().copied())
                .collect();
            // `title` 在这里是小写版,而界面上显示的是原文。**ASCII 长度
            // 相同、只有汉字和少数符号会变长**,所以不能直接拿小写版的
            // 下标去标原文——那会让高亮整体错位。逐段映射回原文
            let orig = &summary.title;
            let title_marks = mark_in(&title, orig, snippet_tokens.iter().copied());
            let (snippet_text, marks) = snippet(&body, snippet_tokens.iter().copied());
            SearchHit {
                summary,
                snippet: snippet_text,
                marks,
                title_marks,
                score,
                fuzzy: false,
            }
        })
        .collect()
}

/// 在 `lower` 里找 `tokens`,把命中的区间映射回 `original` 的**字符**下标。
///
/// **为什么不用 `to_lowercase()` 的结果当区间基准。** 小写化只对 ASCII
/// 有效果,而某些 Unicode 版本里大小写映射会改变长度;土耳其语的
/// `İ` 转小写是两个字符。直接拿小写版的字符数当原文的下标,用户会看见
/// 高亮整体偏一两个字,而且只在特定语言下出现——极难复现
fn mark_in<'t>(
    lower: &str,
    original: &str,
    tokens: impl Iterator<Item = &'t str>,
) -> Vec<[u32; 2]> {
    let orig_chars: Vec<char> = original.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    if orig_chars.len() != lower_chars.len() {
        // 长度对不上就不标。**宁可不高亮,不可标错位置**——标错的比不标的
        // 更糟:用户点进去发现高亮的那段跟说的不是一回事
        return Vec::new();
    }
    let mut marks = Vec::new();
    for t in tokens {
        // **推进量和 find 的返回都用字节。** `str::find` 给的是字节偏移,
        // 曾经把它当字符序号去过 `char_indices().nth(rel)`,那一步会把
        // 字节当字符数,下一个 `lower[from..]` 直接切片越界——是个 panic,
        // 而且只在标题里混了中英文时才现形
        let mut from = 0;
        while let Some(rel) = lower[from..].find(t) {
            let at = from + rel;
            // **字节偏移转字符下标。** 标题在界面上是按字符渲染的,
            // 拿字节下标去切会在半个字上劈开——那是个乱码方块
            let char_at = lower[..at].chars().count();
            marks.push([
                char_at as u32,
                (char_at + t.chars().count()) as u32,
            ]);
            // 推进一个字符(字节),不是 `t.len()`:`t` 是二元组时只覆盖后半个词,
            // 按它推进会整段漏掉相邻的重叠命中
            from = at + lower[at..].chars().next().map(char::len_utf8).unwrap_or(1);
            if from >= lower.len() {
                break;
            }
        }
    }
    marks.sort_unstable();
    marks.dedup();
    marks
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

    // **单个汉字:直接子串判定。** 「苹」在「苹果手机」里,右边紧跟着
    // 另一个汉字,按 ASCII 那套「左右都不能是字母数字」的规则会被判成
    // 不是独立词。汉字本来就没有词边界这回事,判它只能看字在不在
    //
    // 放在循环之前是图早退,不是防错:下面那个循环会**逐个位置**往下试,
    // 试完还是找不到才落到 `false`,所以两个位置的结果完全一样
    if token.chars().count() == 1 && is_cjk(token.chars().next().expect("刚数过一个字")) {
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
fn snippet<'a>(
    body: &str,
    tokens: impl Iterator<Item = &'a str>,
) -> (String, Vec<[u32; 2]>) {
    const WINDOW: usize = 60;

    // body 传进来时已经是小写版,词条也是小写的,直接找。
    // **全都要,不能只取第一个**:正文里"苹果"出现三次,只标第一次的话,
    // 用户看到"这一段里只有一个苹果",他会以为这篇讲的是单一的事
    let wanted: Vec<&str> = tokens.collect();
    // **每个词条的全部出现位置,不是只有第一次。** 少了"全部"这一步,
    // 界面上只标一处,用户看到「这篇文章里只有一个苹果」,会以为它讲的
    // 是单一的事——而正文里明明出现了三次
    let mut hits: Vec<(usize, &str)> = Vec::new();
    for t in &wanted {
        let mut from = 0;
        while let Some(i) = body[from..].find(t) {
            let at = from + i;
            hits.push((at, t));
            // 推进**一个字符**,不是 `t.len()`:`t` 是二元组时只覆盖后半个词,
            // 按它推进会整段漏掉相邻的重叠命中
            from = at + body[at..].chars().next().map(char::len_utf8).unwrap_or(1);
            if from >= body.len() {
                break;
            }
        }
    }
    let Some(&start) = hits.iter().map(|(p, _)| p).min() else {
        return (first_chars(body, WINDOW), Vec::new());
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

    let window = &body[begin..end];
    let slice = window.trim();
    // `trim` 砍掉首尾空白,而区间是从**没 trim 的** window 上算的。
    // 不把这个偏移补回去,每一篇高亮都会恰好偏一点,还极难看出是哪儿错了
    let lead = window.len() - window.trim_start().len();

    let mut out = String::new();
    // **省略号算进下标。** 界面上拼出来的串是"…" + 片段 + "…",区间要是
    // 相对"只有片段"算的,用户看见的高亮会整体偏移一个字
    if begin > 0 {
        out.push('…');
    }
    let prefix_chars = out.chars().count();
    out.push_str(slice);
    if end < body.len() {
        out.push('…');
    }

    let snippet_chars = slice.chars().count();
    let mut marks: Vec<[u32; 2]> = Vec::new();
    for (pos, token) in hits {
        // 落在片段范围外的命中不标。硬标上去只会把高亮推到用户看不见的地方
        if pos < begin + lead || pos >= end {
            continue;
        }
        // **字节差必须转成字符数。** `pos` / `begin` / `lead` 全是字节下标,
        // 直接相减当字符数用,命中词里每有一个汉字就偏一到两个字符——
        // 用户看见的是「高亮的那段跟说的不是一回事」。中文正文里这不是
        // 边角情况,是常态
        let from_chars = prefix_chars + body[begin + lead..pos].chars().count();
        let len = token.chars().count();
        let to_chars = from_chars + len;
        // 越过片段末尾的**宁可漏标**。`trim` 砍掉的那截上如果正好有命中,
        // 界面上去标它就会把 `slice[to]` 越界——那是个 panic,不是一个瑕疵
        if to_chars > snippet_chars {
            continue;
        }
        marks.push([from_chars as u32, to_chars as u32]);
    }
    marks.sort_unstable();
    marks.dedup();
    // **重叠的合并。** 二元组切词天然重叠("数据库"和"据库"都会命中),
    // 不合并的话界面上是两段套着的底色,边上一道难看的接缝
    let mut merged: Vec<[u32; 2]> = Vec::with_capacity(marks.len());
    for m in marks {
        match merged.last_mut() {
            Some(last) if m[0] <= last[1] => last[1] = last[1].max(m[1]),
            _ => merged.push(m),
        }
    }
    (out, merged)
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
                push_cjk_tokens(&text[s..i], &mut tokens);
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
                push_cjk_tokens(&text[s..i], &mut tokens);
            }
        }
    }

    let end = text.len();
    if let Some(s) = ascii_start {
        tokens.push(&text[s..end]);
    }
    if let Some(s) = cjk_start {
        push_cjk_tokens(&text[s..end], &mut tokens);
    }

    // **去重,而且保序。** 加上单字之后,「苹果」这一段会吐出
    // 「苹」「果」「苹果」——二元组的字和单字撞车是必然的。不去掉的话
    // 索引里存的就是一串带重复的词条,白白撑大索引文件
    let mut seen = HashSet::with_capacity(tokens.len());
    tokens.retain(|t| seen.insert(*t));
    tokens
}

/// 把一段连续汉字切成**单字和相邻二字**的切片。
fn push_cjk_tokens<'a>(run: &'a str, out: &mut Vec<&'a str>) {
    // 记的是上一个字的**起始**下标,当前字是它的搭档。记成结束下标的话
    // 会切出「果/手/机」这种丢掉首字的组合
    let mut prev_start: Option<usize> = None;
    for (i, ch) in run.char_indices() {
        let end = i + ch.len_utf8();
        out.push(&run[i..end]);
        if let Some(p) = prev_start {
            out.push(&run[p..end]);
        }
        prev_start = Some(i);
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

/// **高亮的下标得真的落在命中词上。** 这是整套高亮逻辑存在的理由:
    /// 前端是拿 `snippet[from..to]` 去加底色的,下标偏了用户看见的
    /// 就是「高亮的那段跟说的不是一回事」——比不高亮糟糕得多
    #[test]
    fn 高亮区间正正落在命中词上() {
        let body = "前面一段无关的话,后面开始讲苹果的做法,再讲苹果的价格。";
        let lower = body.to_lowercase();
        let (snip, marks) = snippet(&lower, ["苹果"].into_iter());
        assert!(!marks.is_empty(), "该标出命中");
        let chars: Vec<char> = snip.chars().collect();
        for m in &marks {
            let (a, z) = (m[0] as usize, m[1] as usize);
            assert!(z <= chars.len(), "区间越界:{:?} 片段长 {}", m, chars.len());
            let got: String = chars[a..z].iter().collect();
            assert_eq!(got, "苹果", "标出来的应该是命中词,实际是:{}", got);
        }
    }

    /// **下标是字符不是字节。** 中文一个字符三个字节,拿字节下标去切
    /// 会切在半个字上——界面上是一个乱码方块。这类 bug 只在用户剪了中文、
    /// 并且命中词前后有其他中文时才现形,用英文样例测根本碰不到
    #[test]
    fn 高亮下标是字符不是字节() {
        let body = "这是一篇讲数据库索引优化的文章,里面提到数据库很重要。";
        let lower = body.to_lowercase();
        let (snip, marks) = snippet(&lower, ["数据"].into_iter());
        assert!(!marks.is_empty(), "该标出命中");
        let chars: Vec<char> = snip.chars().collect();
        for m in &marks {
            let got: String = chars[m[0] as usize..m[1] as usize].iter().collect();
            assert_eq!(got, "数据", "字符下标切出来的应该是完整词,实际:{}", got);
        }
    }

    /// **词在正文里出现多次,每一处都要标。** 只标第一次的话,用户看到
    /// 「这一段里只有一个苹果」,他会以为这篇讲的是单一的事
    #[test]
    fn 多次出现都标出来() {
        let body = "苹果 香蕉 苹果 橘子 苹果";
        let lower = body.to_lowercase();
        let (snip, marks) = snippet(&lower, ["苹果"].into_iter());
        assert_eq!(marks.len(), 3, "三处都该标上,实际:{:?}", marks);
        let chars: Vec<char> = snip.chars().collect();
        for m in &marks {
            let got: String = chars[m[0] as usize..m[1] as usize].iter().collect();
            assert_eq!(got, "苹果");
        }
    }

    /// **重叠的合并成一段。** 二元组切词天然重叠（「数据库」和「据库」都在）,
    /// 不合并的话界面上是两段套着的底色,边上一道难看的接缝
    #[test]
    fn 重叠的命中合并() {
        let body = "数据库";
        let lower = body.to_lowercase();
        // 「数据」和「据库」重叠在同一个「据」上
        let (snip, marks) = snippet(&lower, ["数据", "据库"].into_iter());
        assert_eq!(marks.len(), 1, "重叠的两段该合成一段,实际:{:?}", marks);
        let chars: Vec<char> = snip.chars().collect();
        let got: String = chars[marks[0][0] as usize..marks[0][1] as usize]
            .iter()
            .collect();
        assert_eq!(got, "数据库", "合并后该覆盖整段,实际:{}", got);
    }

    /// **有省略号时,区间要算上它。** 界面上拼出来的串是「…」+ 片段 + 「…」,
    /// 区间相对「只有片段」算的话,用户看见的高亮会整体偏移一个字
    #[test]
    fn 省略号算进下标() {
        let body = format!("{}苹果{}", "填".repeat(200), "填".repeat(200));
        let lower = body.to_lowercase();
        let (snip, marks) = snippet(&lower, ["苹果"].into_iter());
        assert!(snip.starts_with('…'), "片段该被截断:{}", snip);
        let chars: Vec<char> = snip.chars().collect();
        let got: String = chars[marks[0][0] as usize..marks[0][1] as usize]
            .iter()
            .collect();
        assert_eq!(got, "苹果", "省略号之后的偏移得算进去,实际标的是:{}", got);
    }

    /// **命中落在片段外面就不标。** 硬标上去只会把高亮推到用户看不见的地方。
    /// 这条防的是**越界 panic**:界面上按区间切字符串,越界就是崩
    #[test]
    fn 片段外的命中不标也不越界() {
        let body = format!(
            "{}这里没有那个词{}那个词{}",
            "填".repeat(10),
            "填".repeat(200),
            "填".repeat(200)
        );
        let lower = body.to_lowercase();
        let (snip, marks) = snippet(&lower, ["那个词"].into_iter());
        let chars: Vec<char> = snip.chars().collect();
        for m in &marks {
            assert!(m[1] as usize <= chars.len(), "区间越界:{:?}", m);
        }
    }

    /// **标题的命中区间也是字符下标**,且和界面上显示的**原文**对得上。
    /// 标题在小写化之后可能和原文不是一回事,拿小写版的坐标去标原文,
    /// 高亮会整体错位
    #[test]
    fn 标题命中的区间落在原文上() {
        let title = "Rust Ownership 详解";
        let lower = title.to_lowercase();
        let marks = mark_in(&lower, title, ["ownership"].into_iter());
        assert!(!marks.is_empty(), "该标出命中");
        let chars: Vec<char> = title.chars().collect();
        for m in &marks {
            let got: String = chars[m[0] as usize..m[1] as usize].iter().collect();
            assert_eq!(
                got.to_lowercase(),
                "ownership",
                "标题高亮错位:{}",
                got
            );
        }
    }

    /// **中英混排的标题不 panic。**
    ///
    /// 这条是为一个真 bug 补的:`mark_in` 里把 `str::find` 返回的**字节
    /// 偏移**当成了字符序号去过 `char_indices().nth()`,下一个切片直接越界。
    /// 纯英文标题和纯中文标题都不会触发——字节数恰好等于字符数,两套下标
    /// 退化成同一个值,于是测试全绿。真出问题的是「Rust 所有权」这种中英
    /// 混排的标题,而那恰恰是这个项目最常见的标题形态
    #[test]
    fn 中英混排的标题不panic() {
        let title = "Rust 所有权详解,第二版";
        let lower = title.to_lowercase();
        let marks = mark_in(&lower, title, ["所有权", "rust"].into_iter());
        let chars: Vec<char> = title.chars().collect();
        for m in &marks {
            assert!(
                m[1] as usize <= chars.len(),
                "区间越界:{:?} 标题长 {}",
                m,
                chars.len()
            );
        }
    }

    /// **中文标题里的英文词也得标,而且不能越界。**
    ///
    /// 这条是为一个真 panic 补的:`mark_in` 把 `str::find` 返回的**字节偏移**
    /// 当成了字符序号,下一个 `lower[from..]` 就切在半个字上——
    /// 报的是 `start byte index 9 is not a char boundary`。
    /// 纯英文和纯中文标题都不触发(字节数恰好等于字符数),所以之前全绿;
    /// 真出问题的是「第 199 篇:讲所有权」这种中英混排,而那恰恰是最常见的形态
    #[test]
    fn 中文标题里的英文词不越界() {
        let title = "第 199 篇:讲所有权和借用检查的那点事";
        let lower = title.to_lowercase();
        let marks = mark_in(&lower, title, ["讲所有"].into_iter());
        let chars: Vec<char> = title.chars().collect();
        for m in &marks {
            assert!(
                m[1] as usize <= chars.len(),
                "区间越界:{:?} 标题 {} 字,实际 {:?}",
                m,
                chars.len(),
                title
            );
        }
    }

    /// **大小写映射改变了长度时不标。** 土耳其语的 `İ` 转小写是两个字符,
    /// 原标题和小写版对不上,这时候标出来的必然是错的。宁可不高亮
    #[test]
    fn 长度对不上时不标() {
        let marks = mark_in("ab", "aİb", ["a"].into_iter());
        assert!(marks.is_empty(), "长度对不上时不该标,实际:{:?}", marks);
    }

    /// 测试里绝大多数只关心命中。跳过的那些另有测试盯着,这里不重复断言。
    ///
    /// **不要图省事给 `SearchOutcome` 加个 `is_empty`。** 那等于用一个词
    /// 同时回答"没搜到东西"和"跳过了文件",两件事混在一起之后,
    /// "搜不到任何东西"这个前提就再也验不准了
    fn hits(out: SearchOutcome) -> Vec<SearchHit> {
        out.hits
    }

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

    /// 搜索会跳过读不出的文件——**但不能悄悄跳**。
    ///
    /// 用户明明记得"那篇文章里有这个词",搜出来是空的,而列表视图那边
    /// 已经用红条告诉过他有 3 篇读不出。一搜那 3 篇就没了,一个字都不说,
    /// 他只会以为是自己记错了
    #[test]
    fn 搜索跳过的文件得报出来() {
        let (dir, v) = vault_with(&[("https://a.com/1", "好的一篇", "正文里提到编程")]);
        // 一个压根不是 UTF-8 的文件:读不出来,`read_to_string` 直接失败
        std::fs::write(v.clips_dir().join("坏文件.md"), [0xff, 0xfe, 0x00]).unwrap();

        let out = search(&v, "编程", 10, None).unwrap();

        assert_eq!(out.hits.len(), 1, "好的一篇该搜得到");
        assert_eq!(out.skipped.len(), 1, "读不出的那篇得报出来");
        assert_eq!(out.skipped[0].filename, "坏文件.md");
        drop(dir);
    }

    // ── FTS5 索引 ─────────────────────────────────────────────────────
    //
    // 下面这组测试盯的是 `search_indexed`。索引**只负责挑候选**,
    // 打分和切片段还是走 `search_dir` 原来那套,所以两条路必须给出
    // 逐条相同的结果——不是"差不多",是同一条命中的文件名、顺序、
    // 分数、片段一个字都不能差。
    //
    // 差一点点会怎样:用户搜"苹果",第 3 次能搜到、刷新一下又搜不到,
    // 而且他没有任何办法从界面上看出发生了什么。这比慢可怕得多

    /// 索引路径 / 扫描路径的等价性对照库。
    ///
    /// **故意造出分数相同的结果。** 排序的第一关键字是分数,分数一样时
    /// 退到文件名。两条路哪怕只差一个比较器,也唯有在同分时才看得出来——
    /// 这里必须有同分的条目,否则排序分叉这条最隐蔽的差异会从测试缝里漏过去
    fn 索引对照库() -> (TempDir, Vault) {
        let dir = TempDir::new().expect("建临时目录");
        let v = Vault::new(dir.path());
        let clips = [
            // 标题命中:分最高
            ("https://a.com/1", "苹果发布会", "跟这个主题没关系"),
            // 正文命中:分最低
            ("https://a.com/2", "甲", "这里顺带提了一句苹果手机"),
            // 标题 + 正文:分居中
            ("https://a.com/3", "苹果的十种吃法", "苹果也可以煮汤"),
            // 同分的两条:逼出稳定的次序
            ("https://a.com/4", "乙", "只说一次苹果"),
            ("https://a.com/5", "丙", "也只说一次苹果"),
            // 完全无关:两条路都该滤掉
            ("https://a.com/6", "丁", "讲的是香蕉和橙子"),
        ];
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
        // 每篇都补上同一个标签和同一句批注,让四个字段全部参与打分
        for name in 剪藏文件名(&v) {
            let path = v.clips_dir().join(&name);
            let text = std::fs::read_to_string(&path).expect("刚写的文件读得出来");
            let (block, body) = frontmatter::split(&text).expect("刚写的文件有 frontmatter");
            let mut fm = Frontmatter::parse(block);
            fm.tags = vec!["苹果党".to_string()];
            fm.note = "回头再看看".to_string();
            std::fs::write(&path, fm.to_markdown(body)).expect("写回得成功");
        }
        (dir, v)
    }

    /// **按标题找文件名。** 真实文件名是 `2026-09-30-<id>-<slug>.md`,
    /// 拿标题直接拼文件名是猜的——而猜错的那天,这个测试会红在一个
    /// 完全看不出原因的地方
    fn 标题对应文件名(v: &Vault, title: &str) -> String {
        剪藏文件名(v)
            .into_iter()
            .find(|n| {
                let text = std::fs::read_to_string(v.clips_dir().join(n))
                    .unwrap_or_default();
                frontmatter::split(&text)
                    .map(|(b, _)| Frontmatter::parse(b).title == title)
                    .unwrap_or(false)
            })
            .unwrap_or_else(|| panic!("库里没有标题为「{title}」的那篇"))
    }

    fn 剪藏文件名(v: &Vault) -> Vec<String> {
        std::fs::read_dir(v.clips_dir())
            .expect("剪藏目录读得出来")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".md"))
            .collect()
    }

    fn 索引(v: &Vault) -> crate::index::Index {
        let mut idx = crate::index::Index::open(&v.root().join("对照用索引.sqlite"))
            .expect("建索引");
        // 先空跑一次建库,第二遍才是真走索引(第一遍的活儿都在同步磁盘)
        let _ = search_indexed(&mut idx, &v.clips_dir(), "苹果", 50, None);
        idx
    }

    /// 把一次搜索压成可比对的形状。**分数和片段都必须带上**——只比文件名的话,
    /// 分数算歪了、片段切歪了照样绿,而界面上排序和预览都是错的
    fn 压平(out: &SearchOutcome) -> Vec<(String, i64, String)> {
        out.hits
            .iter()
            .map(|h| (h.summary.filename.clone(), h.score, h.snippet.clone()))
            .collect()
    }

    #[test]
    fn 索引和扫描给出一样的结果() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);

        for query in ["苹果", "苹果 手机", "iPhone", "香蕉", "不存在的词", "苹果党", "甲", "的"] {
            let scanned = search_dir(&v.clips_dir(), query, 50, None).expect("扫描那条路");
            let (indexed, route) = search_indexed(&mut idx, &v.clips_dir(), query, 50, None);

            assert_eq!(route, Route::Index, "搜「{query}」时索引没走,对照就没意义了");
            assert_eq!(
                压平(&indexed),
                压平(&scanned),
                "搜「{query}」两条路给出的结果不一样"
            );
        }
        drop(dir);
    }

    /// 上一条比的是结果,**跳过的文件清单也得一样**。那批红条是用户
    /// 唯一的线索,走索引时少报一个,他就以为那个文件不见了
    #[test]
    fn 索引和扫描报出来的跳过清单一样() {
        let (dir, v) = 索引对照库();
        // 三种跳法各造一个:读不出、没有 frontmatter、有 frontmatter 但没 id
        std::fs::write(v.clips_dir().join("坏文件.md"), [0xff, 0xfe, 0x00]).unwrap();
        std::fs::write(v.clips_dir().join("随手粘的.md"), "今天天气不错,没提苹果。").unwrap();
        std::fs::write(
            v.clips_dir().join("没id的.md"),
            "---\ntitle: \"手工整理的笔记\"\n---\n这里也提到了苹果\n",
        )
        .unwrap();
        let mut idx = 索引(&v);

        let scanned = search_dir(&v.clips_dir(), "苹果", 50, None).expect("扫描");
        let (indexed, _) = search_indexed(&mut idx, &v.clips_dir(), "苹果", 50, None);

        let names = |o: &SearchOutcome| -> Vec<String> {
            o.skipped.iter().map(|s| s.filename.clone()).collect()
        };
        assert_eq!(names(&indexed), names(&scanned), "两条路报出来的跳过清单不一样");
        assert_eq!(names(&scanned).len(), 3, "三种跳法都得算进去");
        drop(dir);
    }

    /// 标签和批注在扫描那条路参与打分,索引那条路也得算。不算的话用户
    /// 按标签/批注搜不到自己亲手写下的东西,而这条差异只在有标签的库上显形
    #[test]
    fn 标签和批注在索引那条路也参与打分() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);

        let (out, route) = search_indexed(&mut idx, &v.clips_dir(), "回头", 50, None);
        assert_eq!(route, Route::Index);
        assert_eq!(out.hits.len(), 6, "批注命中全部,一条都不能少");

        let (out, _) = search_indexed(&mut idx, &v.clips_dir(), "苹果党", 50, None);
        assert_eq!(out.hits.len(), 6, "标签命中全部");
        drop(dir);
    }

    /// 限流得一致,而且**截下来的那一条必须是真·最高分那条**。
    /// 只断言"和扫描一样"的话,两条路一起截错也发现不了,所以再补一条
    /// 不限流时排第一的对照
    #[test]
    fn 索引那条路也照limit截断() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);

        let (full, _) = search_indexed(&mut idx, &v.clips_dir(), "苹果", 50, None);
        let top = full.hits[0].filename().to_string();

        let scanned = search_dir(&v.clips_dir(), "苹果", 1, None).expect("扫描");
        let (indexed, route) = search_indexed(&mut idx, &v.clips_dir(), "苹果", 1, None);
        assert_eq!(route, Route::Index);
        assert_eq!(indexed.hits.len(), 1, "限 1 就该只给一条");
        assert_eq!(indexed.hits[0].filename(), top, "截下来的必须是分数最高那条");
        assert_eq!(压平(&indexed), 压平(&scanned));
        drop(dir);
    }

    /// 分数必须从高到低摆着。索引那条路是**先从 SQLite 拿候选再打分**,
    /// 候选的返回顺序没有任何保证(FTS5 按 bm25,和我们的权重不是一回事),
    /// 漏掉一次排序的话界面上就是"相关度乱七八糟"
    #[test]
    fn 索引那条路的分数是递减的() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);

        let (out, _) = search_indexed(&mut idx, &v.clips_dir(), "苹果", 50, None);
        let scores: Vec<i64> = out.hits.iter().map(|h| h.score).collect();
        let mut sorted = scores.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(scores, sorted, "结果没按分数从高到低排");
        assert!(scores.len() > 2, "样本太少了,排错了也看不出来");
        drop(dir);
    }

    /// 索引**真的**在跑,不是每次都退回扫描。
    ///
    /// 没有这条,`索引和扫描给出一样的结果` 就可能是因为索引那条路一直
    /// 悄悄退化成扫描才绿着的——那才是真正的"看着有、其实没有"
    #[test]
    fn 索引那条路平时真在跑() {
        let (dir, v) = 索引对照库();
        let mut idx = crate::index::Index::open(&v.root().join("跑一下.sqlite")).unwrap();

        let (_, route) = search_indexed(&mut idx, &v.clips_dir(), "苹果", 50, None);
        assert_eq!(route, Route::Index, "第一次就该建好索引并走它");
        assert!(!idx.is_empty(), "索引建好了却还是空的");
        drop(dir);
    }

    /// 改了正文之后要能搜到新的内容,搜不到旧的。
    /// 索引不跟着变是这类模块最经典的 bug:第一次搜建立缓存,用户改完
    /// 再搜还是旧结果,而"重开软件就好了"
    #[test]
    fn 改了文件索引会跟上() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);

        let path = v.clips_dir().join(标题对应文件名(&v, "丁"));
        let text = std::fs::read_to_string(&path).unwrap();
        let (block, _) = frontmatter::split(&text).unwrap();
        let mut fm = Frontmatter::parse(block);
        fm.title = "改过的标题".to_string();
        fm.tags = vec!["柚子".to_string()];
        std::fs::write(&path, fm.to_markdown("现在讲的是柚子")).unwrap();

        let (out, route) = search_indexed(&mut idx, &v.clips_dir(), "柚子", 50, None);
        assert_eq!(route, Route::Index);
        assert_eq!(out.hits.len(), 1, "改完应该搜得到新内容");
        assert_eq!(out.hits[0].filename(), 标题对应文件名(&v, "改过的标题"));

        let (out, _) = search_indexed(&mut idx, &v.clips_dir(), "香蕉", 50, None);
        assert_eq!(out.hits.len(), 0, "旧内容不该还在索引里");
        drop(dir);
    }

    /// 删掉文件之后索引也得跟着删。留着的话用户会搜到一篇已经不在库里的
    /// 文章,点进去报"剪藏不存在"
    #[test]
    fn 文件删了索引里也不能留着() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);
        std::fs::remove_file(v.clips_dir().join(标题对应文件名(&v, "丁"))).unwrap();

        let (out, route) = search_indexed(&mut idx, &v.clips_dir(), "香蕉", 50, None);
        assert_eq!(route, Route::Index);
        assert_eq!(out.hits.len(), 0, "删了的文件不该还搜得到");
        drop(dir);
    }

    /// 索引文件坏了,搜索得能继续用——只是慢一点。
    /// 往那个位置塞一个不是 SQLite 的文件,是最贴近用户真实遭遇的破坏方式
    /// (同步冲突、云盘写坏、自己手滑)
    #[test]
    fn 索引坏了退回扫描而不是报错() {
        let (dir, v) = 索引对照库();
        let path = v.root().join("坏了.sqlite");
        std::fs::write(&path, b"this is not a database at all").unwrap();
        // 生产上走的就是这条:`open` 报错,`open_or_recreate` 删掉重建
        let mut idx = crate::index::Index::open_or_recreate(&path);

        let (out, route) = search_indexed(&mut idx, &v.clips_dir(), "苹果", 50, None);
        assert_eq!(route, Route::Index, "重建之后就该能用索引了");
        assert_eq!(out.hits.len(), 6, "重建之后结果照样是全的");
        assert_eq!(
            crate::index::Index::open(&path).map(|_| ()).map_err(|_| ()),
            Ok(()),
            "重建之后的索引文件该是个好使的 SQLite 库"
        );
        drop(dir);
    }

    /// 空查询是"没搜",不是"全部"。走索引这条路尤其容易搞错——
    /// 索引里躺着一堆东西,直接把候选倒出来看着特别像"全库列表"
    #[test]
    fn 空查询走索引也是空() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);

        for query in ["", "   ", "!!!", "—"] {
            let (out, _) = search_indexed(&mut idx, &v.clips_dir(), query, 50, None);
            assert_eq!(out.hits.len(), 0, "「{query}」不该搜出东西来");
        }
        drop(dir);
    }

    /// 单个汉字要能搜到,而且**两条路都得搜到**。
    ///
    /// 用户搜「豆」是想要「豆包」那几篇。曾经这里断言"搜不到"——
    /// 理由是单字没有区分度,可零结果在用户眼里只有一个解释:搜索坏了
    #[test]
    fn 单个汉字搜得到() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);

        let scanned = search_dir(&v.clips_dir(), "甲", 50, None).expect("扫描");
        let (indexed, route) = search_indexed(&mut idx, &v.clips_dir(), "甲", 50, None);
        assert_eq!(route, Route::Index);
        assert!(!scanned.hits.is_empty(), "库里确实有一篇标题就叫「甲」");
        assert_eq!(
            压平(&indexed),
            压平(&scanned),
            "单字查询两条路给出的结果不一样"
        );
        drop(dir);
    }

    /// 单字查的是**这个字本身**,不是它的近邻。「豆」不该把「逗号」拽出来
    #[test]
    fn 单字查的是这个字() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);

        let (out, _) = search_indexed(&mut idx, &v.clips_dir(), "柚", 50, None);
        // 「柚子」是改文件那条测试里写进去的,这里库是原始的,所以该是 0
        assert_eq!(out.hits.len(), 0, "库里没有「柚」,不该有结果");
        let (out, _) = search_indexed(&mut idx, &v.clips_dir(), "橙", 50, None);
        assert_eq!(out.hits.len(), 1, "「丁」那篇里提过橙子");
        drop(dir);
    }

    /// 库被清空时索引也得跟着空。
    ///
    /// 同步只做"缺的补、变的更新"的时候,清空整个库是个特例:磁盘上一篇
    /// 都没有,那些索引条目一个都不会被碰到。不清的后果是用户清空剪藏后,
    /// 搜索框里还留着刚才那个词,点进去报"已删除的剪藏"——而列表里
    /// 明明一篇都没有,用户完全无从判断这是缓存出了错
    #[test]
    fn 库清空了索引也得跟着空() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);
        assert!(!idx.is_empty(), "先确认索引里确实有东西");

        for name in 剪藏文件名(&v) {
            std::fs::remove_file(v.clips_dir().join(name)).unwrap();
        }

        let (out, _) = search_indexed(&mut idx, &v.clips_dir(), "苹果", 50, None);
        assert_eq!(out.hits.len(), 0, "库里一篇都没有,不该还搜得到");
        assert!(idx.is_empty(), "索引里还留着已经删光的剪藏");
        drop(dir);
    }

    /// **同分的必须按 id 倒序**,不能靠谁先来。id 自带时间序,所以这条
    /// 等价于「新剪的在前」。
    ///
    /// 去掉这条兜底不会立刻出事——扫描那条路和索引那条路送进来的顺序
    /// 恰好一致时,两边都"看着对"。可一旦某次同步让索引的返回顺序变了
    /// (SQLite 按 rowid,也就是插入顺序,和目录枚举顺序没有必然关系),
    /// 同分的两篇就会在刷新之后悄悄换位置。测试没法稳定地造出那个顺序差异,
    /// 所以直接把规则本身钉住
    #[test]
    fn 同分的按id倒序() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);

        let (out, _) = search_indexed(&mut idx, &v.clips_dir(), "只说一次", 50, None);
        assert!(out.hits.len() >= 2, "样本里得有两篇同分的才有得排");
        let ids: Vec<&str> = out.hits.iter().map(|h| h.summary.id.as_str()).collect();
        let mut want = ids.clone();
        want.sort_by(|a, b| b.cmp(a));
        assert_eq!(ids, want, "同分的那几篇没有按 id 倒序排");
        drop(dir);
    }

    // ── 限定搜索范围 ──────────────────────────────────────────────
    //
    // 两三百篇之后「搜苹果」会同时捞出讲手机、谈水果、顺带提一句的
    // 几十篇。用户想的是"标题里有苹果的那几篇",这时候给他一个范围开关
    // 比给他一个更聪明的排序有用
    //
    // **范围只改候选从哪些字段来,不改打分。** 限定标题之后,
    // 标题命中的权重还是 3、标签还是 2——分数在范围之内照样可比。
    // 一旦范围和权重搅在一起,同一个词在不同范围下的排序就没法比较了

    fn 范围对照库() -> (TempDir, Vault) {
        let (dir, v) = 索引对照库();
        // 「丁」那篇只在正文里提了"橙子",标题和标签都没有
        (dir, v)
    }

    /// 只搜标题:正文里提过的搜不到
    #[test]
    fn 限定标题时正文里的命中不算() {
        let (dir, v) = 范围对照库();
        let mut idx = 索引(&v);

        let (out, route) = search_indexed(&mut idx, &v.clips_dir(), "橙", 50, Some(Scope::Title));
        assert_eq!(route, Route::Index);
        assert_eq!(out.hits.len(), 0, "「橙子」只在正文里,限定标题就该搜不到");

        let scanned = search_dir(&v.clips_dir(), "橙", 50, Some(Scope::Title)).unwrap();
        assert_eq!(压平(&out), 压平(&scanned), "两条路在限定标题时不一致");
        drop(dir);
    }

    /// 只搜标签:正文和标题里的命中都不算
    #[test]
    fn 限定标签时只认标签() {
        let (dir, v) = 范围对照库();
        let mut idx = 索引(&v);

        // 对照库里每篇都带「苹果党」标签
        let (out, _) = search_indexed(&mut idx, &v.clips_dir(), "苹果党", 50, Some(Scope::Tag));
        assert_eq!(out.hits.len(), 6, "标签命中全部");

        // 「苹果」在标题正文标签里都有,限定标签之后只剩标签那部分
        let (out, _) = search_indexed(&mut idx, &v.clips_dir(), "橙", 50, Some(Scope::Tag));
        assert_eq!(out.hits.len(), 0, "「橙子」不是标签");
        drop(dir);
    }

    /// 不限定范围时,结果必须和范围是 None 完全一样。
    /// **这是最容易出错的地方**:新加的参数一路传下去,总有一处忘了传,
    /// 于是"默认搜索"悄悄变了行为,而用户只会觉得"搜出来的怎么不一样了"
    #[test]
    fn 不限定范围时和从前一模一样() {
        let (dir, v) = 范围对照库();
        let mut idx = 索引(&v);

        for query in ["苹果", "橙", "回头", "iPhone"] {
            let none = search_indexed(&mut idx, &v.clips_dir(), query, 50, None);
            let any = search_indexed(&mut idx, &v.clips_dir(), query, 50, Some(Scope::Any));
            assert_eq!(压平(&none.0), 压平(&any.0), "搜「{query}」时范围参数改坏了默认行为");
        }
        drop(dir);
    }

    /// **每一种范围,两条路都得一致。** 范围是个枚举,漏掉其中一支
    /// 就会出现"限定批注时索引那路搜得到、扫描那路搜不到"
    #[test]
    fn 每种范围两条路都一致() {
        let (dir, v) = 范围对照库();
        let mut idx = 索引(&v);

        for scope in [Scope::Any, Scope::Title, Scope::Body, Scope::Tag, Scope::Note] {
            for query in ["苹果", "橙", "回头", "苹果党", "不存在的词"] {
                let scanned =
                    search_dir(&v.clips_dir(), query, 50, Some(scope)).expect("扫描");
                let (indexed, route) =
                    search_indexed(&mut idx, &v.clips_dir(), query, 50, Some(scope));
                assert_eq!(route, Route::Index, "{scope:?} 搜「{query}」时索引没走");
                assert_eq!(
                    压平(&indexed),
                    压平(&scanned),
                    "{scope:?} 搜「{query}」两条路不一致"
                );
            }
        }
        drop(dir);
    }

    /// **缩小范围只会少结果,不会多结果。**
    ///
    /// 这是范围唯一真的不变量。原先这里写的是"分数和次序不变",
    /// 写完自己一测就红了:限定标题之后标签那两分就不算了,18 分变成 9 分。
    /// 那是对的——两个范围下的分数本来就不是同一个量,互相比没有意义。
    /// 真正有意义的约束是:限定之后不该冒出宽范围里压根没有的候选,
    /// 那是范围条件写反了(比如在 `Any` 那一支里多加了 `OR`)
    #[test]
    fn 缩小范围只会少结果() {
        let (dir, v) = 范围对照库();
        let mut idx = 索引(&v);

        for query in ["苹果", "橙", "回头", "苹果党", "甲"] {
            let (wide, _) = search_indexed(&mut idx, &v.clips_dir(), query, 50, Some(Scope::Any));
            let wide_names: Vec<&str> = wide.hits.iter().map(|h| h.filename()).collect();

            for scope in [Scope::Title, Scope::Body, Scope::Tag, Scope::Note] {
                let (narrow, _) =
                    search_indexed(&mut idx, &v.clips_dir(), query, 50, Some(scope));
                for h in &narrow.hits {
                    assert!(
                        wide_names.contains(&h.filename()),
                        "搜「{query}」限定 {scope:?} 时冒出了宽范围里没有的候选:{}",
                        h.filename()
                    );
                }
            }
        }
        drop(dir);
    }

    /// 库目录整个不在了(网盘没挂上、剪藏库被挪走),搜索不能炸,
    /// 更不能把"读不出来"说成"没搜到"
    #[test]
    fn 目录没了也不炸() {
        let (dir, v) = 索引对照库();
        let mut idx = 索引(&v);
        let gone = dir.path().join("根本不存在");
        let (out, route) = search_indexed(&mut idx, &gone, "苹果", 50, None);
        assert_eq!(route, Route::Scan);
        assert_eq!(out.hits.len(), 0);
        drop(dir);
    }

    /// 没有 frontmatter 的 `.md` 同样算"跳过"。用户手工粘进来的笔记就是这样,
    /// 它搜不到是合理的,但用户得知道有它存在
    #[test]
    fn 没有frontmatter的文件也得报出来() {
        let (dir, v) = vault_with(&[("https://a.com/1", "好的一篇", "正文里提到编程")]);
        std::fs::write(v.clips_dir().join("随手粘的.md"), "今天天气不错。").unwrap();

        let out = search(&v, "编程", 10, None).unwrap();

        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(out.skipped[0].filename, "随手粘的.md");
        assert!(
            !out.skipped[0].reason.is_empty(),
            "得说清为什么搜不到,不然用户没法判断要不要去修"
        );
        drop(dir);
    }

    /// **没被跳过的就不该报。** 一库好文件里多报几条,红条就成了常态,
    /// 用户会开始无视它——那比不报还糟
    #[test]
    fn 都读得出来时不该报跳过() {
        let (_d, v) = vault_with(&[
            ("https://a.com/1", "甲", "正文里提到编程"),
            ("https://b.com/2", "乙", "正文里也提到编程"),
        ]);
        let out = search(&v, "编程", 10, None).unwrap();
        assert_eq!(out.hits.len(), 2);
        assert!(out.skipped.is_empty(), "一篇都没跳过,不该有任何提示");
    }

    /// 报出来的顺序要稳:用户第二次搜同一个词,看到的清单得和第一次一样
    ///
    /// **在 NTFS 上这条测试观察不到排序有没有真的发生。** NTFS 的目录项按
    /// 文件名建 B 树索引,`read_dir` 枚举出来天生就是排好的(实测:30 个乱序
    /// 写入的文件,读出来照样是排序的)。所以把 `skipped.sort_by(...)` 删掉,
    /// 本机照样绿。ext4 / APFS 枚举出来是哈希序,那边删了就会红——
    /// 这条测试真正保护的是 macOS 和 Linux 的用户。
    ///
    /// 留着它,不因为"本机抓不到变异"就删:它断言的是**正确的性质**,
    /// 不是当前平台碰巧的行为。变异验证在这条上给不出红,那是平台的局限,
    /// 不是测试的假——两回事,别混
    #[test]
    fn 跳过的清单顺序稳定() {
        let (dir, v) = vault_with(&[]);
        // 一个都没存,所以目录压根没建出来。`vault_with` 平时靠 `save` 顺带
        // 建目录,这里得自己建——不然 `write` 直接 NotFound,
        // 报出来的错是"找不到路径"而不是"顺序稳不稳"
        v.ensure_dirs().unwrap();
        for name in ["丙.md", "甲.md", "乙.md"] {
            std::fs::write(v.clips_dir().join(name), "没有 frontmatter。").unwrap();
        }
        let names = |out: &SearchOutcome| -> Vec<String> {
            out.skipped.iter().map(|s| s.filename.clone()).collect()
        };
        let first = search(&v, "编程", 10, None).unwrap();
        let second = search(&v, "编程", 10, None).unwrap();
        // 排的是 **UTF-8 字节序,不是拼音序**:丙 U+4E19 < 乙 U+4E59 < 甲 U+7532。
        // 这条测试第一版照着写入顺序写期望值,红了一次——那说明它当时
        // 根本没有真的在验排序,只是碰巧对上了
        assert_eq!(names(&first), vec!["丙.md", "乙.md", "甲.md"], "按文件名的字节序排");
        assert_eq!(names(&first), names(&second), "两次搜的顺序得一样");
        drop(dir);
    }

    /// 回收站那边同理。回收站里也有读不出的文件(导入失败的那批)
    #[test]
    fn 搜回收站跳过的也得报() {
        let (dir, v) = vault_with(&[("https://a.com/1", "删掉的那篇", "正文里提到编程")]);
        let trashed = v.scan().unwrap().clips[0].filename.clone();
        v.trash(&trashed).unwrap();
        std::fs::write(v.trash_dir().join("回收站里的坏文件.md"), [0xff, 0xfe]).unwrap();

        let out = search_trash(&v, "编程", 10, None).unwrap();

        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(out.skipped[0].filename, "回收站里的坏文件.md");
        drop(dir);
    }

    /// 空查询是"没搜",不是"搜了但什么都没有"。不该顺手报一堆跳过
    #[test]
    fn 空查询不报跳过() {
        let (dir, v) = vault_with(&[("https://a.com/1", "甲", "正文")]);
        std::fs::write(v.clips_dir().join("坏文件.md"), [0xff, 0xfe, 0x00]).unwrap();
        let out = search(&v, "  ", 10, None).unwrap();
        assert!(out.hits.is_empty() && out.skipped.is_empty());
        drop(dir);
    }

    #[test]
    fn 回收站搜的是回收站里的东西() {
        // 界面上回收站是独立视图,搜索框跟着它走。搜的却是库里的东西的话,
        // 用户会以为"我明明删了它怎么还搜得到"——那等于删了个寂寞
        let (_d, v) = vault_with(&[
            ("https://a.com/1", "在库里的那篇", "正文里提到编程这个词"),
            ("https://b.com/2", "删掉的那篇", "正文里同样提到编程这个词"),
        ]);
        let trashed = v
            .scan()
            .unwrap()
            .clips
            .into_iter()
            .find(|c| c.title == "删掉的那篇")
            .unwrap()
            .filename;
        v.trash(&trashed).unwrap();

        let in_lib = hits(search(&v, "编程", 10, None).unwrap());
        assert_eq!(in_lib.len(), 1, "库里那篇不该被算成删掉的");
        assert_eq!(in_lib[0].summary.title, "在库里的那篇");

        let in_trash = hits(search_trash(&v, "编程", 10, None).unwrap());
        assert_eq!(in_trash.len(), 1, "回收站里那篇得搜得到");
        assert_eq!(in_trash[0].summary.title, "删掉的那篇");
    }

    #[test]
    fn 空回收站搜不出东西也不报错() {
        let (_d, v) = vault_with(&[]);
        assert!(hits(search_trash(&v, "编程", 10, None).unwrap()).is_empty());
    }

    #[test]
    fn 两字中文查询能命中() {
        // 这是不用 trigram 的全部理由:trigram 少于三字匹配不到任何行,
        // 「苹果」这种查询会直接返回空
        let (_d, v) = vault_with(&[("https://a.com/1", "手机评测", "这台苹果手机很好用")]);
        let hits = hits(search(&v, "苹果", 10, None).unwrap());
        assert_eq!(hits.len(), 1, "两字中文必须能搜到");
        assert!(hits[0].snippet.contains("苹果"));
    }

    #[test]
    fn 三字及以上的查询也能命中() {
        let (_d, v) = vault_with(&[("https://a.com/1", "深度解析", "我们来看看所有权模型")]);
        assert_eq!(hits(search(&v, "所有权", 10, None).unwrap()).len(), 1);
    }

    #[test]
    fn 英文查询大小写不敏感() {
        let (_d, v) = vault_with(&[("https://a.com/1", "Rust", "Learning Rust ownership")]);
        assert_eq!(hits(search(&v, "rust", 10, None).unwrap()).len(), 1);
        assert_eq!(hits(search(&v, "RUST", 10, None).unwrap()).len(), 1);
    }

    #[test]
    fn 中英混排的查询能拆成两半各自命中() {
        let (_d, v) = vault_with(&[("https://a.com/1", "笔记", "关于 Rust 所有权的笔记")]);
        // 词条是 rust / 所有 / 有权,三个都得在文档里
        assert_eq!(hits(search(&v, "Rust 所有权", 10, None).unwrap()).len(), 1);
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
        let hits = hits(search(&v, "编程", 10, None).unwrap());
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].summary.title, "编程", "标题命中该排前面");
    }

    #[test]
    fn 空查询返回空列表而不是全部() {
        // 空查询当"全部"的话,用户一进搜索框就会看到一堆无关结果
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "正文")]);
        assert!(hits(search(&v, "", 10, None).unwrap()).is_empty());
        assert!(hits(search(&v, "   ", 10, None).unwrap()).is_empty());
    }

    #[test]
    fn 搜不到就老实返回空() {
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "正文")]);
        assert!(hits(search(&v, "量子力学", 10, None).unwrap()).is_empty());
    }

    /// 标签也是能搜的。用户打了「待读」,心里想的是"把待读的那些找出来",
    /// 不是"找出正文里恰好写了待读两个字的那些"。搜不到标签的话,标签栏
    /// 就只剩一个筛子,搜框这一半功能是废的。
    #[test]
    fn 搜得到标签() {
        let (dir, v) = vault_with(&[("https://a.com/1", "标题一", "正文一")]);
        v.set_tags("2026-09-29-1-1-a-com.md", &["待读".into()])
            .ok();
        // 文件名带 id,上面那句多半对不上,换个稳的写法重来
        let _ = dir;
        let (d2, v2) = vault_with(&[("https://b.com/1", "标题二", "正文二")]);
        let name = v2.scan().unwrap().clips[0].filename.clone();
        v2.set_tags(&name, &["待读".into()]).unwrap();

        let hits = hits(search(&v2, "待读", 10, None).unwrap());
        assert_eq!(hits.len(), 1, "标签命中了就得出来");
        assert_eq!(hits[0].summary.title, "标题二");
        drop(d2);
    }

    /// **标题和正文里一个字都不许出现那个词**,只有标签上有——不然这条
    /// 测的就是标题命中,标签压根没参与也照样绿,等于什么都没测。
    #[test]
    fn 只标签不标题不正文也能搜到() {
        let (dir, v) = vault_with(&[("https://c.com/1", "另一篇文章", "完全无关的内容")]);
        let name = v.scan().unwrap().clips[0].filename.clone();
        v.set_tags(&name, &["量子".into()]).unwrap();

        // 先确认它真的搜不到——不然下面的断言可能是别的原因过的
        v.set_tags(&name, &[]).unwrap();
        assert!(
            hits(search(&v, "量子", 10, None).unwrap()).is_empty(),
            "前提:去掉标签之后就该搜不到"
        );

        v.set_tags(&name, &["量子".into()]).unwrap();
        let hits = hits(search(&v, "量子", 10, None).unwrap());
        assert_eq!(hits.len(), 1, "标题正文都没有,只能靠标签命中");
        drop(dir);
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
        assert_eq!(hits(search(&v, "同一个词", 2, None).unwrap()).len(), 2);
    }

    #[test]
    fn 空剪藏库搜索不报错() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        assert!(hits(search(&v, "任何词", 10, None).unwrap()).is_empty());
    }

    #[test]
    fn 分词把汉字切成相邻二字() {
        let tokens = tokenize("苹果手机");
        assert!(tokens.contains(&"苹果"), "实际: {tokens:?}");
        assert!(tokens.contains(&"果手"), "实际: {tokens:?}");
        assert!(tokens.contains(&"手机"), "实际: {tokens:?}");
    }

    /// 单个汉字也要切出来。**以前这里是断言空的**,理由是「查一个字
    /// 必然命中一大堆,没有区分度,不如不查」——可用户不管区分度,
    /// 他搜「豆」是想要「豆包」那几篇,拿到零结果只会有一个结论:搜索坏了
    #[test]
    fn 单个汉字也切得出来() {
        assert_eq!(tokenize("我"), vec!["我"]);
        assert_eq!(tokenize("苹果"), vec!["苹", "果", "苹果"]);
        // 一个字也没有被顺带切成空串之类的东西
        assert!(tokenize("我").iter().all(|t| !t.is_empty()));
    }

    /// 单字和二元组撞车时**只留一个**。「苹果」这段会先吐单字「苹」「果」
    /// 再吐二元组「苹果」,不去重的话索引里存的就是带重复的一串词条,
    /// 白白撑大索引文件
    #[test]
    fn 词条不重复() {
        // 之前这版把 tokens 自己去重之后拿跟自己比——两边恒等,红不了。
        // 真正要盯的是 tokenize 的产出本身有没有重复
        let tokens = tokenize("苹果苹果手机和苹果");
        let mut sorted = tokens.clone();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "tokenize 吐出了重复词条: {tokens:?}");
    }

    #[test]
    fn 标点把汉字段切开() {
        // 「苹果，手机」是两截,不该被拼成「果手」这种跨标点的二元组
        let tokens = tokenize("苹果，手机。");
        assert!(tokens.contains(&"苹果") && tokens.contains(&"手机"));
        assert!(!tokens.contains(&"果手"), "跨标点拼出了二元组");
        assert_eq!(tokens.len(), 6, "两截各出 2 个单字 + 1 个二元组");
    }

    #[test]
    fn 英文查询不命中更长的单词() {
        // 搜 rust 不该把 rustacean 也拽出来——用户搜的是 Rust 这门语言,
        // 不是碰巧含这四个字母的任何词
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "The rustacean is a crab")]);
        assert!(
            hits(search(&v, "rust", 10, None).unwrap()).is_empty(),
            "rust 是 rustacean 的子串,不该算命中"
        );
    }

    #[test]
    fn 英文查询作为独立单词能命中() {
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "Learning Rust ownership")]);
        assert_eq!(hits(search(&v, "rust", 10, None).unwrap()).len(), 1);
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
        let hits = hits(search(&v, "关键", 10, None).unwrap());
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("关键"));
        assert!(
            hits[0].snippet.chars().count() < 200,
            "片段不该把整篇塞进来"
        );
    }

    /// **差一个字就该找得到。** 「手记」和「手机」之间只差一个字,
    /// 这是手滑最可能的样子——用户记得有那篇文章,打错了一个键
    #[test]
    fn 差一个字也算命中() {
        let (d, v) = vault_with(&[("https://a.com/1", "标题", "换了新手机之后的使用感受")]);
        let exact = search(&v, "手机", 10, None).unwrap();
        assert_eq!(exact.hits.len(), 1, "先确认标准搜索本身是好的");

        let loose = search_loose(&v.clips_dir(), "手记", 10, None);
        assert_eq!(loose.len(), 1, "「手记」和「手机」只差一个字,不该搜不到");
        assert!(loose[0].fuzzy, "容错出来的必须标出来,不能混进标准结果里");
        drop(d);
    }

    /// **近似命中一律不高亮。** 我们不知道用户打错的是哪个字,
    /// 标出来等于告诉他"Quire 认定你没打错"——那他更不信这批结果了
    #[test]
    fn 近似命中不标高亮() {
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "换了新手机")]);
        let loose = search_loose(&v.clips_dir(), "手记", 10, None);
        assert_eq!(loose.len(), 1, "先确认这条确实是近似命中");
        assert!(
            loose[0].marks.is_empty(),
            "标了正文高亮:用户打错的是哪个字都不知道,标出来是蒙的"
        );
        assert!(
            loose[0].title_marks.is_empty(),
            "标了标题高亮,同一个道理"
        );
    }

    /// **差两个字不算,但差一个字算——哪怕差的是繁简。**
    ///
    /// 差两个字的多半根本不是同一个词,凑到一起只会让用户以为 Quire 在胡说。
    /// 而繁简只差一个字,算命中是**我们要的**:用户习惯打繁体,就该找得到
    /// 简体的内容,反过来也一样
    #[test]
    fn 差两个字不算差一个字算() {
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "换了新手机")]);
        let dir = v.clips_dir();
        assert!(
            search_loose(&dir, "机器猫", 10, None).is_empty(),
            "差两个字也算命中,那是在编"
        );
        assert!(
            search_loose(&dir, "扫地机器人", 10, None).is_empty(),
            "「换了新手机」和「扫地机器人」之间没有一个二元组差得了一个字,该是空的"
        );
        assert!(
            !search_loose(&dir, "手機", 10, None).is_empty(),
            "繁体和简体只差一个字,该找得到——用户习惯打繁体就照样搜得到简体内容"
        );
    }

    /// **单字不参与容错。** 单字跟任何一个不相干的字距离都 ≤ 1,
    /// 参与进来的后果是"搜一个字,半个库都算近似命中"
    #[test]
    fn 单字不参与容错() {
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "换了新手机")]);
        assert!(
            search_loose(&v.clips_dir(), "手", 10, None).is_empty(),
            "单字进了容错,搜什么都是半个库"
        );
        // **`search_loose` 里也有一道同样的闸**,而上面那条走的正是它——
        // 只测 `search_loose` 的话,底下那道闸改了没人发现。
        // 这里直接问 `loose_window` 自己:它是个 pub 函数,契约得自己守住
        assert_eq!(
            loose_window("换了新手机", "手"),
            None,
            "loose_window 自己没守住「单字不参与」"
        );
    }

    /// **标准搜索已经搜到的时候,一条容错结果都不许混进来。**
    ///
    /// 这条钉的是 `search_with_fallback` 那个"只在全空时才放宽"的判断——
    /// 放宽的位置写错(比如放在"结果少于 N 条就放宽"),用户搜"苹果"会
    /// 得到一堆"苹朵""苹果派",把本来找得到的东西挤出去
    #[test]
    fn 有结果时不混进近似命中() {
        let (_d, v) = vault_with(&[
            ("https://a.com/1", "苹果的用法", "正文一"),
            ("https://a.com/2", "手记本选购", "正文二"),
        ]);
        let dir = v.clips_dir();
        let mut out = search(&v, "苹果", 10, None).unwrap();
        assert_eq!(out.hits.len(), 1);

        // 模拟命令层:有结果就不动它
        if out.hits.is_empty() {
            out.hits = search_loose(&dir, "苹果", 10, None);
        }
        assert!(
            out.hits.iter().all(|h| !h.fuzzy),
            "标准搜索已经命中了,结果里混进了容错出来的"
        );
    }

    /// **超长查询不扫——哪怕它本来是搜得到的。**
    ///
    /// 上一版这条只断言「长查询返回空」,可那条查询压根就搜不到任何东西,
    /// 闸门在不在结果都一样:是条**假绿**。现在让查询**确实对得上**——
    /// 「手机」重复到超过 32 个字,没有闸门的话它就会命中
    #[test]
    fn 超长查询不扫() {
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "换了新手机")]);
        let dir = v.clips_dir();

        // 先证明这个查询本身搜得到——不然下面那个「空」没意义
        assert!(
            !search_loose(&dir, "手机", 10, None).is_empty(),
            "前提不成立:连正常长度的查询都搜不到,这条测试验不了闸门"
        );

        let long = "手机".repeat(20);
        assert!(long.chars().count() > 32);
        assert!(
            search_loose(&dir, &long, 10, None).is_empty(),
            "超长查询没被拦下,那是在全库扫"
        );
    }

    /// **手工放的笔记不进容错结果。** 标准搜索跳过没 id 的文件,
    /// 容错搜索要是把它们放进来,同一个库在两条路上就不是同一个库了
    #[test]
    fn 没id的笔记不进容错() {
        let (_d, v) = vault_with(&[("https://a.com/1", "标题", "换了新手机")]);
        let plain = v.clips_dir().join("2026-01-01-手写笔记.md");
        std::fs::write(&plain, "---\ntitle: 手写笔记\n---\n\n手写的,没有 id\n").unwrap();
        let loose = search_loose(&v.clips_dir(), "手记", 10, None);
        assert!(
            !loose.iter().any(|h| h.summary.title == "手写笔记"),
            "没有 id 的笔记进了容错结果"
        );
    }

    #[test]
    fn 编辑距离一以内才算() {
        let v = |s: &str| s.chars().collect::<Vec<char>>();
        assert!(within_one_edit(&v("手机"), &v("手记")), "换一个字");
        assert!(within_one_edit(&v("手机"), &v("手")), "少一个字");
        assert!(within_one_edit(&v("手机"), &v("新手机")), "前面多一个字");
        assert!(within_one_edit(&v("手机"), &v("手机吧")), "后面多一个字");
        assert!(within_one_edit(&v("手机"), &v("手机")), "完全一样");
        assert!(!within_one_edit(&v("手机"), &v("机器")), "两个字都不一样");
        // 「手机」变「智能手机」是**插了两个字**。看着像"多一个字",
        // 实际差两次——而两次以上的错多半根本不是同一个词,凑出来只会误导
        assert!(!within_one_edit(&v("手机"), &v("智能手机")), "插了两个字不算");
    }

    #[test]
    fn 窗口容错按字符算() {
        // 命中的是"手机"那两个字在正文里的位置,单位是字符不是字节
        assert_eq!(loose_window("换了新手机之后", "手记"), Some(3));
        assert_eq!(loose_window("换了新手机之后", "手基"), Some(3));
        assert_eq!(loose_window("换了新手机之后", "机器猫"), None);
        assert_eq!(loose_window("手", "手记"), None, "比窗口还短");
    }
}

/// 造 N 篇剪藏。放在 tests 模块外,好让性能测试也能用。
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use crate::vault::ClipInput;
    use tempfile::TempDir;

    /// `tests` 模块里那个同名函数的副本:`perf` 是独立模块,拿不到那边的私有项
    pub fn hits(out: super::SearchOutcome) -> Vec<super::SearchHit> {
        out.hits
    }

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
        let hits = tests_support::hits(search(&v, query, 200, None).unwrap());
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
