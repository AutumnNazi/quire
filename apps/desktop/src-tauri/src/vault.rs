//! 剪藏库的文件读写。
//!
//! vault 就是一堆普通 `.md` 文件,躺在用户自己选的目录里。这里做的所有事
//! 都必须满足一个前提:**任何时候删掉这个目录,用户的数据一个字节都不会丢**。
//! 所以没有数据库、没有后台同步,索引(第二周的 FTS5)只是可随时重建的派生物。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, SecondsFormat};
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

fn summary_from(fm: Frontmatter, filename: String) -> ClipSummary {
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
