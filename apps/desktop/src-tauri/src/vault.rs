//! 剪藏库的文件读写。
//!
//! vault 就是一堆普通 `.md` 文件,躺在用户自己选的目录里。这里做的所有事
//! 都必须满足一个前提:**任何时候删掉这个目录,用户的数据一个字节都不会丢**。
//! 所以没有数据库、没有后台同步,搜索是每次现扫文件算出来的。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Local, SecondsFormat};
use serde::{Deserialize, Serialize};

use crate::frontmatter::{self, Frontmatter};
use crate::ids;
use crate::slug;

/// 剪藏文件所在的子目录。留一层目录而不是直接把 `.md` 扔在根下,
/// 是为了以后放附件、导出包时不跟用户的其他文件混在一起。
pub const CLIPS_DIR: &str = "clips";

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("读写剪藏文件失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("文件名不合法,已拒绝: {0}")]
    UnsafeFilename(String),
    #[error("剪藏内容为空,已拒绝")]
    EmptyContent,
    #[error("缺少原文地址")]
    MissingUrl,
    #[error("剪藏不存在: {0}")]
    NotFound(String),
    /// 内部锁在别的线程 panic 时被毒化。此时 vault 状态不可信,
    /// 宁可直接报错让用户重启,也不要拿着半可信状态继续读写用户的文件。
    #[error("内部状态异常,请重启 Quire")]
    Poisoned,
}

/// 扩展 POST 过来的剪藏请求体。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipInput {
    pub schema_version: u32,
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub site_name: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub excerpt: Option<String>,
    pub markdown: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub favicon: Option<String>,
}

/// 列表页要的一条摘要。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipSummary {
    pub id: String,
    pub filename: String,
    pub title: String,
    pub url: String,
    pub site: String,
    pub excerpt: Option<String>,
    pub clipped_at: String,
    pub read: bool,
    pub archived: bool,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreadableFile {
    pub filename: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub clips: Vec<ClipSummary>,
    /// 解析不了的 `.md`。用户可能正手动编辑这些文件,报出来总比默默跳过强。
    pub unreadable: Vec<UnreadableFile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipContent {
    #[serde(flatten)]
    pub summary: ClipSummary,
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedClip {
    pub id: String,
    pub filename: String,
    pub path: String,
}

pub struct Vault {
    root: PathBuf,
}

/// 剪藏库的共享句柄。
///
/// Tauri 命令和剪贴板监控是两条并行访问路径,它们必须看到同一个目录,
/// 否则剪藏存进去、列表读不出来。换成可切换目录时也要它们一起换,所以
/// 用一层锁包住当前指向。
pub type SharedVault = Arc<RwLock<Arc<Vault>>>;

pub fn shared(vault: Vault) -> SharedVault {
    Arc::new(RwLock::new(Arc::new(vault)))
}

impl Vault {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn clips_dir(&self) -> PathBuf {
        self.root.join(CLIPS_DIR)
    }

    pub fn ensure_dirs(&self) -> Result<(), VaultError> {
        fs::create_dir_all(self.clips_dir())?;
        Ok(())
    }

    /// 把一篇剪藏落盘成 `.md`,返回它拿到的 id 与文件名。
    pub fn save(&self, input: &ClipInput) -> Result<SavedClip, VaultError> {
        let body = input.markdown.trim();
        if body.is_empty() {
            return Err(VaultError::EmptyContent);
        }
        let url = input.url.trim();
        if url.is_empty() {
            return Err(VaultError::MissingUrl);
        }
        self.ensure_dirs()?;

        let now = Local::now();
        let id = fresh_id();
        let date = ids::date_prefix(now.year(), now.month(), now.day());

        // 文件名只认 URL 的主机名,不采信扩展传来的 site_name。
        // 扩展在页面缺少 og:site_name 时会把作者名填进那个字段,
        // 照单全收就会得到 "2026-09-28-xxx-张三.md" 这种文件名。
        // 反正主机名我们自己就能从 URL 算出来,没理由去信一个会漂的字段。
        let host = slug::host_of(url);
        let filename = slug::filename_for(&date, &id, &host);

        // site 字段是给人看的,可以容忍扩展传了个显示名
        let site = match input.site_name.trim() {
            "" => host.clone(),
            s => s.to_string(),
        };

        // 标题抽不到是常态(SPA 页面、纯图片文章),拿主机名兜底,
        // 好过列表里一行空白让用户分不清是哪篇
        let title = match input.title.trim() {
            "" => site.clone(),
            t => t.to_string(),
        };

        let fm = Frontmatter {
            id: id.clone(),
            title,
            url: url.to_string(),
            site,
            author: clean_optional(input.author.as_deref()),
            clipped_at: now.to_rfc3339_opts(SecondsFormat::Secs, false),
            published_at: clean_optional(input.published_at.as_deref()),
            excerpt: clean_optional(input.excerpt.as_deref()),
            cover: clean_optional(input.image.as_deref()),
            tags: Vec::new(),
            read: false,
            archived: false,
            extra: Default::default(),
        };

        let path = self.clips_dir().join(&filename);
        write_atomic(&path, fm.to_markdown(body).as_bytes())?;

        Ok(SavedClip {
            id,
            filename: filename.clone(),
            path: format!("{}/{}", CLIPS_DIR, filename),
        })
    }

    /// 扫描整个剪藏库,按剪藏时间倒序返回。索引还没建的第一周,列表就靠这个。
    pub fn scan(&self) -> Result<ScanResult, VaultError> {
        let dir = self.clips_dir();
        if !dir.exists() {
            return Ok(ScanResult::default());
        }

        let mut clips = Vec::new();
        let mut unreadable = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let filename = filename.to_string();
            match read_summary(&path, &filename) {
                Ok(summary) => clips.push(summary),
                Err(reason) => unreadable.push(UnreadableFile { filename, reason }),
            }
        }

        // id 自带时间序,直接按它倒排就是"新剪的在前",不用再读 clipped_at
        clips.sort_by(|a, b| b.id.cmp(&a.id));
        unreadable.sort_by(|a, b| a.filename.cmp(&b.filename));
        Ok(ScanResult { clips, unreadable })
    }

    /// 改已读 / 归档标志。传 `None` 表示这一项不动。
    ///
    /// **重写整个文件是有代价的,所以必须做到只改该改的。** 用户的剪藏文件
    /// 归用户所有:正文里的空行、行尾空格、他自己加的字段、记事本存出来的
    /// CRLF 换行,一样都不能动。动一样,Quire 就成了那个"存下来其实是租的"
    /// 的工具——只是租给了 Quire 自己。
    ///
    /// 做法是:原文拆成 frontmatter 段和正文段,**只重新序列化 frontmatter**,
    /// 正文原样拼回去;换行风格按原文头部判断,原样还原。
    pub fn set_flags(
        &self,
        filename: &str,
        read: Option<bool>,
        archived: Option<bool>,
    ) -> Result<ClipSummary, VaultError> {
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        let path = self.clips_dir().join(filename);
        let original = fs::read_to_string(&path)
            .map_err(|_| VaultError::NotFound(filename.to_string()))?;
        let (block, body) = frontmatter::split(&original)
            .ok_or_else(|| VaultError::NotFound(filename.to_string()))?;

        // 头部用 CRLF 就整份还原成 CRLF。判据看 frontmatter 段自身,
        // 因为正文里混着两种换行也不算用户手滑。
        let newline = if block.contains("\r\n") { "\r\n" } else { "\n" };

        let mut fm = Frontmatter::parse(block);
        if let Some(v) = read {
            fm.read = v;
        }
        if let Some(v) = archived {
            fm.archived = v;
        }

        // 分隔符后**只跟一个换行**:split 切出来的 body 自带原来那个空行,
        // 再补一个就等于每次改标志给正文加一行,文件会越滚越胖。
        let mut out = fm.render().replace('\n', newline);
        out = format!("---{newline}{out}---{newline}{body}");
        write_atomic(&path, out.as_bytes())?;

        Ok(summary_from(fm, filename.to_string()))
    }

    /// 按 ISO 自然周汇总,最近的一周在最前,最多取 `weeks` 周。
    ///
    /// **用 ISO 周而不是「最近七天」**,因为回顾要的是"我第几周剪了几篇",
    /// 一个滚动窗口没法回答这个问题——每周一打开软件看到的分组都不一样。
    ///
    /// 跨年那周是 ISO 规则的经典坑:归属年由**包含该周星期四**的那一年决定。
    /// 2026-01-01 是周四,属于 2026 年第 1 周,不是 2025 年第 53 周。
    /// 判错了用户元旦剪的东西会落到去年年底那栏里。交给 chrono 的
    /// `iso_week`,自己手搓 `第几周 = (day_of_year + 6) / 7` 一定会错。
    pub fn weekly_digest(&self, weeks: usize) -> Result<Vec<WeekDigest>, VaultError> {
        let dir = self.clips_dir();
        if !dir.exists() || weeks == 0 {
            return Ok(Vec::new());
        }

        let mut buckets: BTreeMap<(i32, u32), WeekDigest> = BTreeMap::new();
        for entry in fs::read_dir(&dir)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Some((block, _)) = frontmatter::split(&text) else { continue };
            let fm = Frontmatter::parse(block);
            if fm.id.is_empty() {
                continue;
            }
            let Some(key) = iso_week_key(&fm.clipped_at) else {
                continue;
            };
            let b = buckets.entry(key).or_insert_with(|| WeekDigest {
                iso_year: key.0,
                iso_week: key.1,
                total: 0,
                unread: 0,
                read: 0,
                clips: Vec::new(),
            });
            b.total += 1;
            if fm.read {
                b.read += 1;
            } else {
                b.unread += 1;
            }
            b.clips.push(fm.title);
        }

        let mut out: Vec<WeekDigest> = buckets.into_values().collect();
        // BTreeMap 是按 (年, 周) 升序排的,反过来才是"最近的在最前"
        out.sort_by(|a, b| b.iso_year.cmp(&a.iso_year).then_with(|| b.iso_week.cmp(&a.iso_week)));
        out.truncate(weeks);
        Ok(out)
    }

    /// 把整个剪藏库拼成**一个** Markdown 文件。
    ///
    /// 刻意不做 zip、不做 json、不做任何自家格式。理由很直接:README 上写着
    /// "数据是你的",那导出的东西就必须**脱离 Quire 也能读**。用户拿这个文件
    /// 丢进 Obsidian、Logseq、Notion 或者任何一个编辑器,都该是能直接看的东西。
    /// 做成 zip 就等于把用户的数据再关一次锁,那和"租"没区别。
    ///
    /// 结构是「开头一张索引表 + 每篇一节」,每节之间用 `---` 分开。**不带
    /// frontmatter**——每篇都带 YAML 的话,拼起来会有十几个 `---` 分隔线,
    /// 在别的 Markdown 工具里会被当成十几个文档的边界,标题层级全乱。
    pub fn export_markdown(&self) -> Result<String, VaultError> {
        let dir = self.clips_dir();
        if !dir.exists() {
            return Ok(empty_export());
        }

        // 先收集再排序:目录遍历顺序是随机的,直接边走边拼的话导出文件
        // 每次生成都不一样,没法做版本对比。
        let mut items: Vec<(String, Frontmatter, String)> = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            // 读不出来的跳过而不是整体失败:用户手动改坏的 .md 不该让
            // 整份导出泡汤——那等于因为一个错文件拿不回全部数据。
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Some((block, body)) = frontmatter::split(&text) else { continue };
            let fm = Frontmatter::parse(block);
            if fm.id.is_empty() {
                continue;
            }
            items.push((fm.clipped_at.clone(), fm, body.trim().to_string()));
        }
        if items.is_empty() {
            return Ok(empty_export());
        }
        // 和列表一致:新剪的在前。id 自带时间序,同秒内也不会乱。
        items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));

        let mut out = String::new();
        out.push_str("# Quire 剪藏导出\n\n");
        out.push_str(&format!(
            "共 {} 篇,由 Quire 导出。\n\n",
            items.len()
        ));
        out.push_str("| 剪藏于 | 标题 | 原文 |\n| --- | --- | --- |\n");
        for (clipped_at, fm, _) in &items {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                clipped_at,
                escape_cell(&fm.title),
                fm.url
            ));
        }
        out.push_str("\n---\n");

        for (_, fm, body) in &items {
            out.push_str(&format!("\n## {}\n\n", fm.title.trim()));
            let mut meta = vec![format!("- 剪藏于 {}", fm.clipped_at)];
            if !fm.site.is_empty() {
                meta.push(format!("- 来源 {}", fm.site));
            }
            if let Some(author) = fm.author.as_deref().filter(|s| !s.is_empty()) {
                meta.push(format!("- 作者 {}", author));
            }
            if !fm.url.is_empty() {
                meta.push(format!("- 原文 <{}>", fm.url));
            }
            out.push_str(&meta.join("\n"));
            out.push_str("\n\n");
            out.push_str(body);
            out.push('\n');
        }
        Ok(out)
    }

    /// 按文件名取正文。文件名来自前端,必须先过白名单。
    pub fn read_clip(&self, filename: &str) -> Result<ClipContent, VaultError> {
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        let path = self.clips_dir().join(filename);
        let text = fs::read_to_string(&path).map_err(|_| VaultError::NotFound(filename.to_string()))?;
        let (block, body) = frontmatter::split(&text)
            .ok_or_else(|| VaultError::NotFound(filename.to_string()))?;
        let fm = Frontmatter::parse(block);
        Ok(ClipContent {
            summary: summary_from(fm, filename.to_string()),
            body: body.trim_start().to_string(),
        })
    }
}

/// 一周的汇总,给「每周回顾」用。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeekDigest {
    /// ISO 周年,比如 2026。注意不是跨年那一周的日历年。
    pub iso_year: i32,
    /// ISO 周序号 1..=53。
    pub iso_week: u32,
    pub total: usize,
    pub unread: usize,
    pub read: usize,
    pub clips: Vec<String>,
}

/// 从 `clipped_at`(ISO 8601 带时区)取出 ISO 周年和周序号。
///
/// 存的是带偏移的本地时间,得**按本地时间**归周:晚上 11 点剪的东西
/// 属于"我剪的那天"所在的那周,不是 UTC 那天。chrono 解析出的
/// `DateTime<FixedOffset>` 保留原偏移,直接拿它问 iso_week 就行。
fn iso_week_key(clipped_at: &str) -> Option<(i32, u32)> {
    let parsed = DateTime::parse_from_rfc3339(clipped_at).ok()?;
    let iso = parsed.iso_week();
    Some((iso.year(), iso.week()))
}

/// 表格单元格里不能出现裸的 `|`,否则整张表会错位。换行会截断这一行,
/// 所以也换成空格——标题里的换行在索引表里没意义。
fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\r', '\n'], " ")
}

/// 空库导出的内容。用户点了导出就该拿到一个能打开的文件,
/// 哪怕里面明说还没有剪藏,总比报错让人以为出了故障强。
fn empty_export() -> String {
    "# Quire 剪藏导出\n\n这个剪藏库还是空的,还没有剪藏。\n".to_string()
}

fn clean_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// 当前时刻的 id。盐值取纳秒位:同毫秒内两次剪藏也不会撞。
fn fresh_id() -> String {
    let salt = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    ids::new_id(Local::now().timestamp_millis(), salt)
}

/// 由 frontmatter 拼出列表摘要。搜索模块也要用,所以是 pub。
pub fn summary_from(fm: Frontmatter, filename: String) -> ClipSummary {
    ClipSummary {
        id: fm.id,
        filename,
        title: fm.title,
        url: fm.url,
        site: fm.site,
        excerpt: fm.excerpt,
        clipped_at: fm.clipped_at,
        read: fm.read,
        archived: fm.archived,
        tags: fm.tags,
    }
}

fn read_summary(path: &Path, filename: &str) -> Result<ClipSummary, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let (block, _) = frontmatter::split(&text).ok_or("缺少 frontmatter 头部")?;
    let fm = Frontmatter::parse(block);
    if fm.id.is_empty() {
        return Err("frontmatter 缺少 id".to_string());
    }
    Ok(summary_from(fm, filename.to_string()))
}

/// 先写临时文件再改名,避免进程被杀掉时留下半截剪藏。
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, bytes)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            // 撞名只可能是盐值碰撞,概率极低。Windows 的 rename 不覆盖已存在文件,
            // 所以删掉目标重试,好过留下一个写坏的剪藏。
            if path.exists() {
                fs::remove_file(path)?;
                fs::rename(&tmp, path)?;
                Ok(())
            } else {
                Err(VaultError::Io(first_err))
            }
        }
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn input(url: &str, title: &str, md: &str) -> ClipInput {
        ClipInput {
            schema_version: 1,
            url: url.to_string(),
            title: title.to_string(),
            site_name: String::new(),
            author: None,
            excerpt: None,
            markdown: md.to_string(),
            published_at: None,
            image: None,
            favicon: None,
        }
    }

    fn vault() -> (TempDir, Vault) {
        let dir = TempDir::new().expect("建临时目录");
        let v = Vault::new(dir.path());
        (dir, v)
    }

    #[test]
    fn 保存后能从列表读回来() {
        let (_d, v) = vault();
        let saved = v.save(&input(
            "https://example.com/post/1",
            "深入理解所有权",
            "# 深入理解所有权\n\n正文内容。",
        ))
        .expect("应保存成功");

        assert!(saved.filename.starts_with("2026-"), "文件名应带日期前缀: {}", saved.filename);
        assert!(saved.filename.ends_with("example-com.md"));

        let scan = v.scan().expect("应扫描成功");
        assert_eq!(scan.clips.len(), 1);
        assert_eq!(scan.clips[0].title, "深入理解所有权");
        assert_eq!(scan.clips[0].url, "https://example.com/post/1");
        assert!(!scan.clips[0].read);
    }

    #[test]
    fn 详情能取回不含frontmatter的正文() {
        let (_d, v) = vault();
        let saved = v
            .save(&input("https://a.com/x", "标题", "# 标题\n\n第一段。\n\n第二段。"))
            .unwrap();
        let content = v.read_clip(&saved.filename).expect("应读到正文");
        assert!(content.body.starts_with("# 标题"));
        assert!(!content.body.contains("id:"), "正文里不该混进 frontmatter");
        assert!(!content.body.contains("---"));
    }

    #[test]
    fn 列表按剪藏时间倒序() {
        let (_d, v) = vault();
        for i in 0..3 {
            v.save(&input(&format!("https://a{}.com/", i), &format!("第{}篇", i), "正文")).unwrap();
            // 同一毫秒内连续保存时,靠盐值保证 id 不重复;这里退一步也不该崩
        }
        let scan = v.scan().unwrap();
        assert_eq!(scan.clips.len(), 3);
        // id 倒序排完,任意两两之间不应出现逆序
        for w in scan.clips.windows(2) {
            assert!(w[0].id >= w[1].id, "列表未按 id 倒序: {} < {}", w[0].id, w[1].id);
        }
    }

    #[test]
    fn 拒绝路径穿越的文件名() {
        let (_d, v) = vault();
        // 详情接口按文件名取正文,这条不守等于把用户整个磁盘开放出去
        let err = v.read_clip("../../../Windows/System32/config/SAM.md").unwrap_err();
        assert!(matches!(err, VaultError::UnsafeFilename(_)));
    }

    #[test]
    fn 拒绝空内容和缺URL() {
        let (_d, v) = vault();
        assert!(matches!(
            v.save(&input("https://a.com/", "标题", "   \n  ")).unwrap_err(),
            VaultError::EmptyContent
        ));
        assert!(matches!(
            v.save(&input("  ", "标题", "正文")).unwrap_err(),
            VaultError::MissingUrl
        ));
    }

    #[test]
    fn 抽不到标题时用主机名兜底() {
        // SPA 和纯图片文章抽不出标题是常态,列表里不能是一片空白
        let (_d, v) = vault();
        v.save(&input("https://news.example.com/story", "", "正文")).unwrap();
        let scan = v.scan().unwrap();
        assert_eq!(scan.clips[0].title, "news.example.com");
    }

    #[test]
    fn 文件名基于URL主机名而非站点名() {
        // 回归测试:页面缺 og:site_name 时,扩展会把作者名塞进 siteName。
        // 早先直接拿它命名,用户看到的文件名就是 "2026-09-28-xxx-张三.md"
        let (_d, v) = vault();
        let mut inp = input("https://news.example.com/post", "标题", "正文");
        inp.site_name = "张三".to_string();
        let saved = v.save(&inp).unwrap();
        assert!(
            saved.filename.ends_with("-news-example-com.md"),
            "文件名应基于主机名,实际: {}",
            saved.filename
        );
        // site 字段是给人看的,显示名可以保留
        assert_eq!(v.scan().unwrap().clips[0].site, "张三");
    }

    #[test]
    fn 坏文件被报告而不是静默丢弃() {
        // 用户可能正手动编辑 vault 里的文件。悄悄跳过会让人以为剪藏丢了。
        let (_d, v) = vault();
        v.save(&input("https://a.com/1", "正常文章", "正文")).unwrap();
        fs::write(v.clips_dir().join("broken.md"), "这里没有 frontmatter").unwrap();
        fs::write(v.clips_dir().join("note.txt"), "不是 md").unwrap();

        let scan = v.scan().unwrap();
        assert_eq!(scan.clips.len(), 1, "正常文章应被读到");
        assert_eq!(scan.unreadable.len(), 1, "坏文件应被报告");
        assert_eq!(scan.unreadable[0].filename, "broken.md");
    }

    #[test]
    fn 用户手写的CRLF文件能被读出() {
        let (_d, v) = vault();
        v.save(&input("https://a.com/1", "标题", "正文")).unwrap();
        let path = v.clips_dir().join("manual.md");
        fs::write(
            &path,
            "---\r\nid: \"manual01\"\r\ntitle: \"手写的\"\r\nurl: \"https://a.com/\"\r\nsite: \"a.com\"\r\n---\r\n\r\n正文\r\n",
        )
        .unwrap();

        let scan = v.scan().unwrap();
        assert!(scan.clips.iter().any(|c| c.title == "手写的"), "CRLF 文件应能解析");
    }

    #[test]
    fn 改已读状态不碰正文和用户自己的字段() {
        // 这条测试守的是「数据是用户的」这条底线。改一个布尔值就把用户手写的
        // 自定义字段、换行风格、正文里的空行全洗掉的话,Quire 就成了那个
        // 「存下来其实是租的」的工具——只是租给了 Quire 自己。
        let (_d, v) = vault();
        v.save(&input("https://a.com/1", "标题", "第一段\n\n第二段  \n缩进")).unwrap();
        // 文件名是 save 自己算的(日期+id+主机名),别在这儿猜——猜错了
        // 测试会报「文件不存在」,跟被测的逻辑八竿子打不着
        let name = v.scan().unwrap().clips[0].filename.clone();
        let path = v.clips_dir().join(&name);
        let original = fs::read_to_string(&path).unwrap();
        let body_before = frontmatter::split(&original).unwrap().1.to_string();

        v.set_flags(&name, Some(true), None).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        let (block, body_after) = frontmatter::split(&after).unwrap();
        assert_eq!(body_after, body_before, "正文必须逐字节不变");
        assert!(
            body_after.contains("第一段\n\n第二段  \n缩进"),
            "正文里的空行和行尾空格不能被规范化"
        );
        let fm = Frontmatter::parse(block);
        assert!(fm.read, "read 应改成 true");
        assert!(!fm.archived, "只传 read 时 archived 不该被动到");
        // 正文内容不变,文件的总长度也只应该差在 frontmatter 那一行
        assert!(after.ends_with(&body_before), "文件应以原正文结尾");
    }

    #[test]
    fn 改已读状态保留用户手写的未知字段() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let path = v.clips_dir().join("mine.md");
        fs::write(
            &path,
            "---\nid: \"mine0001\"\ntitle: \"我的\"\nurl: \"https://a.com/\"\nsite: \"a.com\"\nrating: 5\nstatus: \"在读\"\n---\n\n正文\n",
        )
        .unwrap();

        v.set_flags("mine.md", Some(true), None).unwrap();

        let rewritten = fs::read_to_string(&path).unwrap();
        let (block, _) = frontmatter::split(&rewritten).unwrap();
        let fm = Frontmatter::parse(block);
        assert!(fm.extra.contains_key("rating"), "rating 是用户自己加的,不能丢");
        assert!(fm.extra.contains_key("status"), "status 同理");
    }

    #[test]
    fn 改已读状态保留CRLF换行风格() {
        // to_markdown 只吐 LF。用户从记事本存的文件是 CRLF,改一次标志就
        // 把它整个转成 LF,等于 Quire 擅自改了用户文件的字节。
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let path = v.clips_dir().join("crlf.md");
        fs::write(
            &path,
            "---\r\nid: \"crlf0001\"\r\ntitle: \"记事本\"\r\nurl: \"https://a.com/\"\r\nsite: \"a.com\"\r\nread: false\r\n---\r\n\r\n正文\r\n",
        )
        .unwrap();
        let before = fs::read_to_string(&path).unwrap();
        let body_before = frontmatter::split(&before).unwrap().1.to_string();

        v.set_flags("crlf.md", Some(true), None).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        let (block, body_after) = frontmatter::split(&after).unwrap();
        assert_eq!(body_after, body_before, "正文必须逐字节不变");
        assert!(body_after.contains('\r'), "CRLF 文件的正文也要保住 \\r");
        // 精确判据:文件里每一个 \n 前面都得有 \r。早先写的是
        // `!after.contains("\n正文")`,那句是错的——"\r\n正文" 本身就
        // 包含 "\n正文",一条 CRLF 文件也会把它判红。
        let bare_lf = after
            .char_indices()
            .any(|(i, c)| c == '\n' && (i == 0 || !after[..i].ends_with('\r')));
        assert!(!bare_lf, "不该出现裸 LF,那说明换行风格被改写了");
        assert!(Frontmatter::parse(block).read, "标志本身还是要改对");
    }

    #[test]
    fn 改已读状态拒绝路径穿越() {
        let (_d, v) = vault();
        assert!(v.set_flags("../../evil.md", Some(true), None).is_err(), "不能写到 vault 外面");
    }

    #[test]
    fn 周回顾把剪藏按自然周分组() {
        // 边界最容易错的是跨年那一周。2026-01-01 是周四,它属于 2026 年的
        // 第 1 周(ISO 规则:包含该周星期四的那一年才是归属年),不是 2025
        // 年的第 53 周。判断错了,用户元旦剪的东西会跑到去年年底那栏里。
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        for (id, title, when) in [
            ("m0000001", "元旦剪的", "2026-01-01T09:00:00+08:00"),
            ("m0000002", "周日剪的", "2026-01-04T09:00:00+08:00"),
            ("m0000003", "下周一剪的", "2026-01-05T09:00:00+08:00"),
        ] {
            fs::write(
                v.clips_dir().join(format!("{id}.md")),
                format!("---\nid: \"{id}\"\ntitle: \"{title}\"\nurl: \"https://a.com/\"\nsite: \"a.com\"\nclipped_at: \"{when}\"\nread: false\n---\n\n正文\n"),
            )
            .unwrap();
        }

        let weeks = v.weekly_digest(8).unwrap();
        assert_eq!(weeks.len(), 2, "元旦那周和下周一应该分成两周");
        assert!(weeks[0].clips.contains(&"下周一剪的".to_string()), "最近的一周排在最前");
        assert!(weeks[1].clips.contains(&"元旦剪的".to_string()));
        assert!(weeks[1].clips.contains(&"周日剪的".to_string()), "周日应和元旦同属一周");
        assert_eq!(weeks[0].unread, 1, "没读过的要数出来,这是回顾的重点");
    }

    #[test]
    fn 周回顾数得清已读和未读() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        fs::write(
            v.clips_dir().join("a.md"),
            "---\nid: \"aaaa0001\"\ntitle: \"读过的\"\nurl: \"https://a.com/\"\nsite: \"a.com\"\nclipped_at: \"2026-03-02T09:00:00+08:00\"\nread: true\n---\n\n正文\n",
        )
        .unwrap();
        fs::write(
            v.clips_dir().join("b.md"),
            "---\nid: \"bbbb0001\"\ntitle: \"没读的\"\nurl: \"https://b.com/\"\nsite: \"b.com\"\nclipped_at: \"2026-03-02T10:00:00+08:00\"\nread: false\n---\n\n正文\n",
        )
        .unwrap();

        let weeks = v.weekly_digest(8).unwrap();
        assert_eq!(weeks[0].total, 2);
        assert_eq!(weeks[0].unread, 1);
        assert_eq!(weeks[0].read, 1);
    }

    #[test]
    fn 周回顾按周数截断() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        // 三个相隔一周的周一,分属三个 ISO 周
        for (i, day) in ["2026-03-02", "2026-03-09", "2026-03-16"].iter().enumerate() {
            fs::write(
                v.clips_dir().join(format!("w{i}.md")),
                format!("---\nid: \"w00000{i}\"\ntitle: \"第{i}周\"\nurl: \"https://a.com/\"\nsite: \"a.com\"\nclipped_at: \"{day}T09:00:00+08:00\"\n---\n\n正文\n"),
            )
            .unwrap();
        }

        assert_eq!(v.weekly_digest(1).unwrap().len(), 1, "只取最近一周");
        assert_eq!(v.weekly_digest(10).unwrap().len(), 3, "周数够就全给");
    }

    #[test]
    fn 周回顾不把读不出来的文件算进去() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        fs::write(v.clips_dir().join("bad.md"), "没有 frontmatter").unwrap();
        assert!(v.weekly_digest(8).unwrap().is_empty(), "坏文件不该造出一周来");
    }

    #[test]
    fn 导出是纯Markdown且自带清单() {
        // 「数据是你的」不能只是一句口号。导出的东西必须**用别的工具也读得动**,
        // 所以是纯 Markdown,不是 zip、不是 json、不是自家格式。用户拿这个文件
        // 丢进 Obsidian / Logseq / 任何编辑器,都得是能看的东西。
        let (_d, v) = vault();
        v.save(&input("https://a.com/1", "第一篇", "正文一")).unwrap();
        v.save(&input("https://b.com/2", "第二篇", "正文二")).unwrap();

        let out = v.export_markdown().unwrap();
        assert!(out.contains("| 标题 |"), "开头应有索引表格");
        assert!(out.contains("第一篇"), "清单里应有第一篇");
        assert!(out.contains("第二篇"));
        assert!(out.contains("正文一"), "正文不能只导标题——那等于只导了目录");
        assert!(out.contains("https://a.com/1"), "原文地址必须留着,否则回溯链断了");
    }

    #[test]
    fn 导出按剪藏时间从新到旧() {
        // 和列表一致。用户导出是为了快速翻阅,顺序反了就没法用。
        let (_d, v) = vault();
        v.save(&input("https://a.com/1", "旧", "旧正文")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        v.save(&input("https://b.com/2", "新", "新正文")).unwrap();

        let out = v.export_markdown().unwrap();
        let new_at = out.find("## 新").expect("应有一节标题为「新」");
        let old_at = out.find("## 旧").expect("应有一节标题为「旧」");
        assert!(new_at < old_at, "新剪的应排在旧的前面");
    }

    #[test]
    fn 导出不含frontmatter标记() {
        // 每个 clip 自带 YAML 块的话,拼起来的文件会有十几个 --- 分隔线,
        // 在别的 Markdown 工具里会被当成十几个文档的边界,标题层级全乱。
        let (_d, v) = vault();
        v.save(&input("https://a.com/1", "标题", "正文")).unwrap();

        let out = v.export_markdown().unwrap();
        // 首尾各一个 hr 分隔整篇,正文里不该再出现 --- 行
        let hr_lines = out
            .lines()
            .filter(|l| l.trim() == "---")
            .count();
        assert_eq!(hr_lines, 1, "只该有开头那一道分隔线,不该把每个 clip 的 YAML 也带进来");
        assert!(!out.contains("clipped_at:"), "frontmatter 字段不该出现在导出里");
    }

    #[test]
    fn 空库导出仍然给出可用文件() {
        // 用户点了导出就该拿到一个文件,哪怕里面写着"还没有剪藏"。
        // 直接报错等于让用户以为出了故障。
        let (_d, v) = vault();
        let out = v.export_markdown().unwrap();
        assert!(!out.trim().is_empty(), "空库也要导出点东西出来");
    }

    #[test]
    fn 导出跳过读不出来的文件而不是整体失败() {
        // vault 里可能有用户手动改坏的 .md。一篇坏的不能让整份导出泡汤。
        let (_d, v) = vault();
        v.save(&input("https://a.com/1", "好的", "正文")).unwrap();
        v.ensure_dirs().unwrap();
        fs::write(v.clips_dir().join("broken.md"), "这个文件根本没有 frontmatter").unwrap();

        let out = v.export_markdown().unwrap();
        assert!(out.contains("好的"), "正常的那篇必须还在");
    }

    #[test]
    fn 空vault扫描不报错() {
        // 首次启动、用户还没剪过任何东西,列表页要能正常打开
        let (_d, v) = vault();
        let scan = v.scan().expect("空目录不该报错");
        assert!(scan.clips.is_empty());
        assert!(scan.unreadable.is_empty());
    }

    #[test]
    fn 同名文件不会互相覆盖() {
        // 连续剪同一篇文章,两次都得留下来
        let (_d, v) = vault();
        let a = v.save(&input("https://a.com/p", "标题", "第一次")).unwrap();
        let b = v.save(&input("https://a.com/p", "标题", "第二次")).unwrap();
        assert_ne!(a.filename, b.filename, "两次剪藏不应共用文件名");
        assert_eq!(v.scan().unwrap().clips.len(), 2);
    }
}
