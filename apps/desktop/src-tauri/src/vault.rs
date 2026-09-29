//! 剪藏库的文件读写。
//!
//! vault 就是一堆普通 `.md` 文件,躺在用户自己选的目录里。这里做的所有事
//! 都必须满足一个前提:**任何时候删掉这个目录,用户的数据一个字节都不会丢**。
//! 所以没有数据库、没有后台同步,搜索是每次现扫文件算出来的。

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

/// 回收站。藏在 clips/ 下面而不是 vault 根部,是因为 `scan()` 只读 clips/ 一层,
/// 点开头的目录 Obsidian 之类也会自动隐藏——用户平时看不见它,要用的时候找得到。
/// `scan()` 靠的是"扩展名不是 .md 就跳过",目录天然落选,不需要额外判断。
pub const TRASH_DIR: &str = ".trash";

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("读写剪藏文件失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("文件名不合法,已拒绝: {0}")]
    UnsafeFilename(String),
    #[error("剪藏内容为空,已拒绝")]
    EmptyContent,
    #[error("剪藏不存在: {0}")]
    NotFound(String),
    #[error("已经有同名剪藏了,没敢放回去: {0}")]
    AlreadyExists(String),
    #[error("回收站里挤不下了,请自己清一清: {0}")]
    TrashFull(String),
    /// 文件搬回去了但元数据解析失败。**文件已经回到库里了**——撤销是让用户
    /// 拿回东西的,不能因为读不出元数据就反悔把它留在回收站里。
    #[error("剪藏已放回,但读不出元数据: {0}({1})")]
    Unreadable(String, String),
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

/// 存完之后的结果。**「已经剪过了」是一个正常的结局,不是错误。**
///
/// 早先一律往库里塞,同一篇文章存两遍,列表里就多一条一模一样的。
/// 用户得自己认出哪条是新的——那是在让用户替软件擦屁股。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SaveOutcome {
    Saved { id: String, filename: String },
    Duplicate { filename: String, title: String },
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

        // site 字段是给人看的,可以容忍扩展传了个显示名。
        // 剪贴板里没有链接是常态——随手复制一段话、一个命令、一段代码。
        // 那种剪藏老实标成"剪贴板":站点栏空着的话,列表里就是
        // " · 09-29 16:20"这么一行,看着像程序坏了。
        let site = match input.site_name.trim() {
            "" if host.is_empty() => "剪贴板".to_string(),
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

    /// 存一篇剪藏,存之前先看库里有没有同一篇。
    ///
    /// `force` 是给用户的出口:文章更新了想重存一份是合理需求,判重要是
    /// 拦死不让存,用户就只剩"自己去剪藏目录里改文件名"这一条路。
    /// 界面在提示重复时必须给得出这个选项。
    ///
    /// 判重只读不写——已有那篇的正文一个字都不该动(见 `判重时不碰已有那篇的正文`)。
    pub fn save_checked(&self, input: &ClipInput, force: bool) -> Result<SaveOutcome, VaultError> {
        if !force {
            if let Some(existing) = self.find_by_url(&input.url)? {
                return Ok(SaveOutcome::Duplicate {
                    filename: existing.filename,
                    title: existing.title,
                });
            }
        }
        let saved = self.save(input)?;
        Ok(SaveOutcome::Saved {
            id: saved.id,
            filename: saved.filename,
        })
    }

    /// 按原文地址找已经剪过的那一篇。
    ///
    /// **一篇东西在同一篇文章上存两份,是没人在意的重复,却实实在在把列表
    /// 弄脏了。** 识别一篇文章的凭据是它的地址:标题可能一模一样,正文
    /// 可能被网站改过,地址不会。
    pub fn find_by_url(&self, url: &str) -> Result<Option<ClipSummary>, VaultError> {
        let want = normalize_url(url);
        if want.is_empty() {
            // 剪贴板里没有链接的纯文本也会存。拿空串去全库比对毫无意义,
            // 而且真撞上了就是"所有剪藏都算重复"
            return Ok(None);
        }
        Ok(self
            .scan()?
            .clips
            .into_iter()
            .find(|c| normalize_url(&c.url) == want))
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
        let original =
            fs::read_to_string(&path).map_err(|_| VaultError::NotFound(filename.to_string()))?;
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
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Some((block, body)) = frontmatter::split(&text) else {
                continue;
            };
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
        out.push_str(&format!("共 {} 篇,由 Quire 导出。\n\n", items.len()));
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

    /// 回收站。**点错了不该找不回来**,这是剪藏工具里唯一一个能把用户东西
    /// 弄没的操作,所以它不删文件,只搬到 `clips/.trash/`。用户后悔了可以
    /// 撤销,没撤销也还能自己去剪藏目录里把文件捞出来。
    pub fn trash_dir(&self) -> PathBuf {
        self.clips_dir().join(TRASH_DIR)
    }

    /// 把一篇剪藏移进回收站。文件**搬走**而不是复制,搬完原位置就没了。
    pub fn trash(&self, filename: &str) -> Result<(), VaultError> {
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        let from = self.clips_dir().join(filename);
        if !from.is_file() {
            return Err(VaultError::NotFound(filename.to_string()));
        }
        // **id 要在搬之前读。** 文件一旦挪进回收站,`id_of` 就读不到了,
        // 图片会原地不动,删一篇就在 assets/ 里留一窝孤儿图
        let id = self.id_of(filename);
        fs::create_dir_all(self.trash_dir())?;
        // 回收站里重名不能覆盖。剪藏文件名是时间+随机 id 撞上的概率极低,
        // 但"极低"不是"不会"——悄悄覆盖掉用户的东西是最不能忍的一类错。
        fs::rename(&from, self.free_trash_slot(filename)?)?;
        if let Some(id) = id {
            self.move_assets(&id, true)?;
        }
        Ok(())
    }

    /// 从回收站放回原位。返回放回去之后的摘要,省得前端再全量扫一次盘。
    pub fn restore(&self, filename: &str) -> Result<ClipSummary, VaultError> {
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        let from = self.trash_dir().join(filename);
        if !from.is_file() {
            return Err(VaultError::NotFound(filename.to_string()));
        }
        let to = self.clips_dir().join(filename);
        // 原位已经有同名的(用户手工放回来过),那就别动它,报冲突
        if to.exists() {
            return Err(VaultError::AlreadyExists(filename.to_string()));
        }
        let id = self.id_of_in(&self.trash_dir().join(filename));
        fs::create_dir_all(self.clips_dir())?;
        fs::rename(&from, &to)?;
        if let Some(id) = id {
            self.move_assets(&id, false)?;
        }
        read_summary(&to, filename)
            .map_err(|reason| VaultError::Unreadable(filename.to_string(), reason))
    }

    /// 把一篇剪藏的图片目录搬进回收站 / 搬回来。找不到就当没有——
    /// 还没做图片本地化的剪藏本来就没有这个目录,不该因此报错。
    fn move_assets(&self, id: &str, to_trash: bool) -> Result<(), VaultError> {
        let live = self.assets_dir().join(id);
        let trashed = self.trash_dir().join(crate::assets::ASSETS_DIR).join(id);
        let (from, to) = if to_trash {
            (&live, &trashed)
        } else {
            (&trashed, &live)
        };
        if !from.is_dir() {
            return Ok(());
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(from, to)?;
        Ok(())
    }

    /// 读出剪藏的 id。图片目录按 id 命名,搬图片时要用。
    fn id_of(&self, filename: &str) -> Option<String> {
        self.id_of_in(&self.clips_dir().join(filename))
    }

    fn id_of_in(&self, path: &Path) -> Option<String> {
        let text = fs::read_to_string(path).ok()?;
        let (block, _) = frontmatter::split(&text)?;
        let id = Frontmatter::parse(block).id;
        if id.is_empty() {
            None
        } else {
            Some(id)
        }
    }

    /// 图片目录。平铺在 clips/ 下面、按剪藏 id 分桶。`scan()` 只看
    /// `clips/` 一层且只认 `.md`,所以它天然不落进列表。
    pub fn assets_dir(&self) -> PathBuf {
        self.clips_dir().join(crate::assets::ASSETS_DIR)
    }

    /// 把一篇剪藏里的远程图片下到本地,并把正文里的地址换成相对路径。
    ///
    /// **调用方负责把它放到后台线程。** 这一步可能要下几十兆、走好几秒,
    /// 挡在保存流程前面的话,`Ctrl+V` 一下等三秒,这个工具就没人用了。
    pub fn localize_images<F>(&self, filename: &str, fetch: F) -> Result<(usize, usize), String>
    where
        F: Fn(&str) -> Result<(Option<String>, Vec<u8>), String>,
    {
        if !slug::is_safe_filename(filename) {
            return Err(format!("文件名不合法: {filename}"));
        }
        let path = self.clips_dir().join(filename);
        let text = fs::read_to_string(&path).map_err(|e| format!("读不出剪藏: {e}"))?;
        let (block, body) =
            frontmatter::split(&text).ok_or_else(|| format!("剪藏格式不对: {filename}"))?;
        let id = Frontmatter::parse(block).id;

        let (new_body, ok, failed) = crate::assets::localize(body, &id, &self.assets_dir(), fetch)?;
        // 一张都没换成功就别动文件,省掉一次无谓的写盘
        if new_body == body {
            return Ok((ok, failed));
        }
        // **frontmatter 一个字节都不碰。** 只把正文那一段换掉——它是从
        // 原文尾部切出来的,前面那截原样拼回去,用户的字段、换行风格、
        // 字段顺序都不会被顺带"整理"掉。
        let head = &text[..text.len() - body.len()];
        write_atomic(&path, format!("{head}{new_body}").as_bytes()).map_err(|e| e.to_string())?;
        Ok((ok, failed))
    }

    /// 在回收站里给 `filename` 找一个没人占的坑位,最多试 100 次。
    fn free_trash_slot(&self, filename: &str) -> Result<PathBuf, VaultError> {
        let dir = self.trash_dir();
        let candidate = dir.join(filename);
        if !candidate.exists() {
            return Ok(candidate);
        }
        for n in 1..=100 {
            let alt = dir.join(format!("{n}-{filename}"));
            if !alt.exists() {
                return Ok(alt);
            }
        }
        Err(VaultError::TrashFull(filename.to_string()))
    }

    /// 按文件名取正文。文件名来自前端,必须先过白名单。
    pub fn read_clip(&self, filename: &str) -> Result<ClipContent, VaultError> {
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        let path = self.clips_dir().join(filename);
        let text =
            fs::read_to_string(&path).map_err(|_| VaultError::NotFound(filename.to_string()))?;
        let (block, body) =
            frontmatter::split(&text).ok_or_else(|| VaultError::NotFound(filename.to_string()))?;
        let fm = Frontmatter::parse(block);
        Ok(ClipContent {
            summary: summary_from(fm, filename.to_string()),
            body: body.trim_start().to_string(),
        })
    }
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
/// 地址归一化到"够用来判重"的程度。**刻意只做最小的那点处理**:大小写
/// 敏感的路径不能一律小写,查询串里的参数更不能丢——那是两个不同页面。
/// 只吃掉尾斜杠和首尾空白,剩下的原样比。
fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // 只去尾部连续的斜杠,根路径 `https://a.com/` 不能被削成 `https://a.com`
    let without = trimmed.trim_end_matches('/');
    if without.len()
        >= trimmed
            .split("://")
            .next()
            .map(|s| s.len() + 3)
            .unwrap_or(0)
    {
        without.to_string()
    } else {
        trimmed.to_string()
    }
}

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
    fn 删掉的剪藏从列表里消失但文件还在() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "该删的", "正文")).unwrap();

        v.trash(&saved.filename).unwrap();

        assert!(v.scan().unwrap().clips.is_empty(), "列表里不该再出现");
        assert!(
            !v.clips_dir().join(&saved.filename).exists(),
            "原位置不该留着"
        );
        assert!(
            v.trash_dir().join(&saved.filename).exists(),
            "文件应搬到回收站"
        );
        // 递归数一遍:是"搬走了",不是"复制了一份"
        assert_eq!(walk_all(v.root()).len(), 1, "vault 里只该剩这一份文件");
        drop(dir);
    }

    #[test]
    fn 撤销能把剪藏放回原位() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "后悔了", "正文")).unwrap();

        v.trash(&saved.filename).unwrap();
        let back = v.restore(&saved.filename).unwrap();

        assert_eq!(back.filename, saved.filename);
        assert_eq!(back.title, "后悔了");
        assert_eq!(v.scan().unwrap().clips.len(), 1);
        assert!(!v.trash_dir().join(&saved.filename).exists());
        drop(dir);
    }

    #[test]
    fn 回收站不会出现在列表里() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "留着", "正文")).unwrap();
        let b = v.save(&input("https://b.com", "扔掉", "正文")).unwrap();
        v.trash(&b.filename).unwrap();

        let names: Vec<String> = v
            .scan()
            .unwrap()
            .clips
            .into_iter()
            .map(|c| c.filename)
            .collect();
        assert_eq!(names, vec![a.filename]);
        drop(dir);
    }

    #[test]
    fn 回收站里重名不会覆盖前一个() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "第一个", "正文")).unwrap();
        // 绕过 trash 往回收站塞一个同名文件,模拟"删过一轮、又被手工放回来删第二轮"
        fs::create_dir_all(v.trash_dir()).unwrap();
        fs::write(v.trash_dir().join(&a.filename), "占位".as_bytes()).unwrap();

        v.trash(&a.filename).unwrap();

        // 原来那个占位文件必须还在。悄悄覆盖掉用户的东西是最不能忍的一类错
        assert_eq!(
            fs::read_to_string(v.trash_dir().join(&a.filename)).unwrap(),
            "占位"
        );
        let names: Vec<String> = fs::read_dir(v.trash_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".md"))
            .collect();
        assert_eq!(names.len(), 2, "回收站里该有两份,不是一份被覆盖掉");
        drop(dir);
    }

    #[test]
    fn 删除和撤销都挡路径穿越() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        for bad in ["../evil.md", "a/b.md", r"..\evil.md"] {
            assert!(
                matches!(v.trash(bad), Err(VaultError::UnsafeFilename(_))),
                "{bad} 不该被放行"
            );
            assert!(
                matches!(v.restore(bad), Err(VaultError::UnsafeFilename(_))),
                "{bad} 不该被放行"
            );
        }
        drop(dir);
    }

    #[test]
    fn 删不存在的剪藏要说人话() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let missing = "20260101-aaaaaaaa-none-com.md";
        assert!(matches!(v.trash(missing), Err(VaultError::NotFound(_))));
        assert!(matches!(v.restore(missing), Err(VaultError::NotFound(_))));
        drop(dir);
    }

    #[test]
    fn 图片落到本地后正文指向它() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input(
                "https://a.com/post",
                "带图的",
                "![封面](https://cdn.example.com/a.png)\n\n正文。\n",
            ))
            .unwrap();

        let (ok, failed) = v
            .localize_images(&saved.filename, |url| {
                assert!(url.starts_with("https://cdn.example.com/"), "{url}");
                Ok((Some("image/png".to_string()), b"\x89PNG fake".to_vec()))
            })
            .unwrap();

        assert_eq!((ok, failed), (1, 0));
        let text = fs::read_to_string(v.clips_dir().join(&saved.filename)).unwrap();
        assert!(text.contains("assets/"), "正文该指向本地图片,实际:\n{text}");
        assert!(
            !text.contains("cdn.example.com"),
            "不该还留着远程地址:\n{text}"
        );
        assert!(
            v.assets_dir().join(&saved.id).join("0.png").exists(),
            "图片得真的落盘"
        );
        assert!(text.contains("正文。"), "正文别的地方不能动");
        drop(dir);
    }

    #[test]
    fn 图片本地化不能动frontmatter() {
        // set_flags 那条规矩在这里同样成立:只换正文,头部一个字节都不碰
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let path = {
            let saved = v
                .save(&input(
                    "https://a.com/post",
                    "带图的",
                    "![a](https://cdn.x.com/a.png)\n",
                ))
                .unwrap();
            // 手工塞一个用户自己的字段,模拟"用户在 Quire 之外改过文件"
            let p = v.clips_dir().join(&saved.filename);
            let text = fs::read_to_string(&p)
                .unwrap()
                .replace("tags: []\n", "tags: [手写的]\nmine: 1\n");
            fs::write(&p, text).unwrap();
            let head = fs::read_to_string(&p)
                .unwrap()
                .split("---")
                .nth(1)
                .unwrap()
                .to_string();
            v.localize_images(&saved.filename, |_| {
                Ok((Some("image/png".to_string()), b"x".to_vec()))
            })
            .unwrap();
            let after = fs::read_to_string(&p).unwrap();
            assert_eq!(
                after.split("---").nth(1).unwrap(),
                head,
                "frontmatter 被动了:\n{after}"
            );
            p
        };
        assert!(path.exists());
        drop(dir);
    }

    #[test]
    fn 图片下不下来时正文原封不动() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input(
                "https://a.com/post",
                "下不来",
                "![a](https://cdn.x.com/a.png)\n",
            ))
            .unwrap();
        let before = fs::read_to_string(v.clips_dir().join(&saved.filename)).unwrap();

        let (ok, failed) = v
            .localize_images(&saved.filename, |_| Err("连不上".to_string()))
            .unwrap();

        assert_eq!((ok, failed), (0, 1));
        assert_eq!(
            fs::read_to_string(v.clips_dir().join(&saved.filename)).unwrap(),
            before,
            "一张都没下成就不该碰用户的文件"
        );
        assert!(!v.assets_dir().join(&saved.id).exists(), "不该留下空目录");
        drop(dir);
    }

    #[test]
    fn 删剪藏时图片跟着走() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input(
                "https://a.com/post",
                "带图的",
                "![a](https://cdn.x.com/a.png)\n",
            ))
            .unwrap();
        v.localize_images(&saved.filename, |_| {
            Ok((Some("image/png".into()), b"x".to_vec()))
        })
        .unwrap();
        assert!(v.assets_dir().join(&saved.id).join("0.png").exists());

        v.trash(&saved.filename).unwrap();
        assert!(
            !v.assets_dir().join(&saved.id).exists(),
            "图片该跟着进回收站,别留孤儿"
        );
        assert!(v
            .trash_dir()
            .join(crate::assets::ASSETS_DIR)
            .join(&saved.id)
            .join("0.png")
            .exists());

        v.restore(&saved.filename).unwrap();
        assert!(
            v.assets_dir().join(&saved.id).join("0.png").exists(),
            "撤销时图片也得回来"
        );
        assert!(!v
            .trash_dir()
            .join(crate::assets::ASSETS_DIR)
            .join(&saved.id)
            .exists());
        drop(dir);
    }

    #[test]
    fn 回收站里没有图片的剪藏删起来照样正常() {
        // 手工放进去的 .md、没经过图片本地化的老剪藏,都没有 assets 目录
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input("https://a.com/post", "老剪藏", "没有图。\n"))
            .unwrap();
        assert!(v.trash(&saved.filename).is_ok());
        assert!(v.restore(&saved.filename).is_ok());
        drop(dir);
    }

    #[test]
    fn 图片目录不该混进列表() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input(
                "https://a.com/post",
                "带图的",
                "![a](https://cdn.x.com/a.png)\n",
            ))
            .unwrap();
        v.localize_images(&saved.filename, |_| {
            Ok((Some("image/png".into()), b"x".to_vec()))
        })
        .unwrap();

        let names: Vec<String> = v
            .scan()
            .unwrap()
            .clips
            .into_iter()
            .map(|c| c.filename)
            .collect();
        assert_eq!(names, vec![saved.filename], "assets/ 目录不该出现在列表里");
        assert!(
            v.scan().unwrap().unreadable.is_empty(),
            "assets/ 也不该被报成读不出元数据"
        );
        drop(dir);
    }

    #[test]
    fn 认得出已经剪过的同一篇() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com/post", "第一遍", "正文"))
            .unwrap();

        let hit = v.find_by_url("https://a.com/post").unwrap();

        assert_eq!(hit.map(|c| c.title), Some("第一遍".to_string()));
        drop(dir);
    }

    #[test]
    fn 不同文章不算重复() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com/post-1", "甲", "正文"))
            .unwrap();
        v.save(&input("https://a.com/post-2", "乙", "正文"))
            .unwrap();

        assert!(v.find_by_url("https://a.com/post-3").unwrap().is_none());
        // 前缀相同的两个地址不能被当成同一篇
        assert!(v.find_by_url("https://a.com/post").unwrap().is_none());
        drop(dir);
    }

    #[test]
    fn 末尾多个斜杠算同一篇() {
        // 分享链接带不带尾斜杠是随意的,判成两篇纯粹恶心人
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com/post/", "甲", "正文")).unwrap();

        assert!(v.find_by_url("https://a.com/post").unwrap().is_some());
        assert!(v.find_by_url("  https://a.com/post//  ").unwrap().is_some());
        drop(dir);
    }

    #[test]
    fn 空地址不去库里找() {
        // 剪贴板里没有链接的纯文本也会存,拿空串去全库比对毫无意义
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com/post", "甲", "正文")).unwrap();

        assert!(v.find_by_url("   ").unwrap().is_none());
        drop(dir);
    }

    #[test]
    fn 读不出元数据的文件不会把查询搞崩() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com/post", "甲", "正文")).unwrap();
        fs::write(v.clips_dir().join("坏文件.md"), "没有 frontmatter").unwrap();

        assert!(v.find_by_url("https://a.com/post").unwrap().is_some());
        drop(dir);
    }

    /// 递归数一遍 vault 里有多少个文件,用来证明"搬走了"而不是"复制了一份"。
    fn walk_all(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out
    }

    #[test]
    fn 保存后能从列表读回来() {
        let (_d, v) = vault();
        let saved = v
            .save(&input(
                "https://example.com/post/1",
                "深入理解所有权",
                "# 深入理解所有权\n\n正文内容。",
            ))
            .expect("应保存成功");

        assert!(
            saved.filename.starts_with("2026-"),
            "文件名应带日期前缀: {}",
            saved.filename
        );
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
            .save(&input(
                "https://a.com/x",
                "标题",
                "# 标题\n\n第一段。\n\n第二段。",
            ))
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
            v.save(&input(
                &format!("https://a{}.com/", i),
                &format!("第{}篇", i),
                "正文",
            ))
            .unwrap();
            // 同一毫秒内连续保存时,靠盐值保证 id 不重复;这里退一步也不该崩
        }
        let scan = v.scan().unwrap();
        assert_eq!(scan.clips.len(), 3);
        // id 倒序排完,任意两两之间不应出现逆序
        for w in scan.clips.windows(2) {
            assert!(
                w[0].id >= w[1].id,
                "列表未按 id 倒序: {} < {}",
                w[0].id,
                w[1].id
            );
        }
    }

    #[test]
    fn 拒绝路径穿越的文件名() {
        let (_d, v) = vault();
        // 详情接口按文件名取正文,这条不守等于把用户整个磁盘开放出去
        let err = v
            .read_clip("../../../Windows/System32/config/SAM.md")
            .unwrap_err();
        assert!(matches!(err, VaultError::UnsafeFilename(_)));
    }

    #[test]
    fn 拒绝空内容() {
        let (_d, v) = vault();
        assert!(matches!(
            v.save(&input("https://a.com/", "标题", "   \n  "))
                .unwrap_err(),
            VaultError::EmptyContent
        ));
    }

    #[test]
    fn 没有原文地址也能存() {
        // 「随手复制一段话」是 Quire 最常见的用法。用户按了 Ctrl+V 却只看到
        // 一句「缺少原文地址」、什么都没存上——主路径被堵死,零门槛也就没了
        let (dir, v) = vault();
        let saved = v
            .save(&input(
                "",
                "随手记的一个点子",
                "今天想到:剪藏工具应该只存 Markdown",
            ))
            .unwrap();
        assert!(dir.path().join("clips").join(&saved.filename).exists());
    }

    #[test]
    fn 没有地址的剪藏站点标成剪贴板() {
        // 站点栏空着的话,列表里就是" · 09-29 16:20"这么一行,看着像坏了
        let (_d, v) = vault();
        let saved = v.save(&input("", "随手记的一个点子", "正文")).unwrap();
        let clip = v
            .read_clip(&saved.filename)
            .expect("刚存的剪藏应能读回来")
            .summary;
        assert_eq!(clip.site, "剪贴板");
        assert_eq!(clip.url, "", "没有地址就老实留空,别编一个假链接出来");
        assert!(
            saved.filename.ends_with("-clipped.md"),
            "文件名后缀要能看:{}",
            saved.filename
        );
    }

    #[test]
    fn 没有地址的永远不算重复() {
        // 两段毫不相干的话,地址都是空的。判重要是拿空串去比对,
        // 结果就是"第二段话永远存不进去"——那是静默丢数据,比多存一份糟得多
        let (_d, v) = vault();
        v.save(&input("", "第一段", "今天天气不错")).unwrap();
        v.save(&input("", "第二段", "顺便记个电话")).unwrap();
        assert_eq!(v.scan().unwrap().clips.len(), 2, "两段都得在");
    }

    #[test]
    fn 抽不到标题时用主机名兜底() {
        // SPA 和纯图片文章抽不出标题是常态,列表里不能是一片空白
        let (_d, v) = vault();
        v.save(&input("https://news.example.com/story", "", "正文"))
            .unwrap();
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
        v.save(&input("https://a.com/1", "正常文章", "正文"))
            .unwrap();
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
        assert!(
            scan.clips.iter().any(|c| c.title == "手写的"),
            "CRLF 文件应能解析"
        );
    }

    #[test]
    fn 改已读状态不碰正文和用户自己的字段() {
        // 这条测试守的是「数据是用户的」这条底线。改一个布尔值就把用户手写的
        // 自定义字段、换行风格、正文里的空行全洗掉的话,Quire 就成了那个
        // 「存下来其实是租的」的工具——只是租给了 Quire 自己。
        let (_d, v) = vault();
        v.save(&input(
            "https://a.com/1",
            "标题",
            "第一段\n\n第二段  \n缩进",
        ))
        .unwrap();
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
        assert!(
            fm.extra.contains_key("rating"),
            "rating 是用户自己加的,不能丢"
        );
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
        assert!(
            v.set_flags("../../evil.md", Some(true), None).is_err(),
            "不能写到 vault 外面"
        );
    }

    #[test]
    fn 导出是纯Markdown且自带清单() {
        // 「数据是你的」不能只是一句口号。导出的东西必须**用别的工具也读得动**,
        // 所以是纯 Markdown,不是 zip、不是 json、不是自家格式。用户拿这个文件
        // 丢进 Obsidian / Logseq / 任何编辑器,都得是能看的东西。
        let (_d, v) = vault();
        v.save(&input("https://a.com/1", "第一篇", "正文一"))
            .unwrap();
        v.save(&input("https://b.com/2", "第二篇", "正文二"))
            .unwrap();

        let out = v.export_markdown().unwrap();
        assert!(out.contains("| 标题 |"), "开头应有索引表格");
        assert!(out.contains("第一篇"), "清单里应有第一篇");
        assert!(out.contains("第二篇"));
        assert!(out.contains("正文一"), "正文不能只导标题——那等于只导了目录");
        assert!(
            out.contains("https://a.com/1"),
            "原文地址必须留着,否则回溯链断了"
        );
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
        let hr_lines = out.lines().filter(|l| l.trim() == "---").count();
        assert_eq!(
            hr_lines, 1,
            "只该有开头那一道分隔线,不该把每个 clip 的 YAML 也带进来"
        );
        assert!(
            !out.contains("clipped_at:"),
            "frontmatter 字段不该出现在导出里"
        );
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
        fs::write(
            v.clips_dir().join("broken.md"),
            "这个文件根本没有 frontmatter",
        )
        .unwrap();

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

    #[test]
    fn 存之前判重_重复的只报已有那篇() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let first = v
            .save(&input("https://a.com/post", "第一遍", "原正文"))
            .unwrap();

        // 末尾多个斜杠指的是同一篇,不该当成两篇
        let again = input("https://a.com/post/", "第二遍", "新正文");
        match v.save_checked(&again, false).unwrap() {
            SaveOutcome::Duplicate { filename, title } => {
                assert_eq!(filename, first.filename, "要报的是库里那篇,不是刚剪的");
                assert_eq!(title, "第一遍");
            }
            SaveOutcome::Saved { .. } => panic!("同一篇地址不该存第二份"),
        }
        assert_eq!(v.scan().unwrap().clips.len(), 1, "库里应该还是一篇");
    }

    #[test]
    fn 判重时不碰已有那篇的正文() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let first = v
            .save(&input("https://a.com/post", "标题", "原正文"))
            .unwrap();
        let before =
            std::fs::read_to_string(dir.path().join("clips").join(&first.filename)).unwrap();

        v.save_checked(&input("https://a.com/post", "新标题", "新正文"), false)
            .unwrap();

        let after =
            std::fs::read_to_string(dir.path().join("clips").join(&first.filename)).unwrap();
        assert_eq!(before, after, "判重是只读的,不该把已有剪藏重写一遍");
    }

    #[test]
    fn 用户坚持就照存() {
        // 文章更新了想重存一份,这是合理需求。判重不能变成死胡同——
        // 不给出口的话,用户唯一能做的就是自己去剪藏目录里改文件名。
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com/post", "第一遍", "原正文"))
            .unwrap();

        let again = input("https://a.com/post", "第二遍", "新正文");
        match v.save_checked(&again, true).unwrap() {
            SaveOutcome::Saved { filename, .. } => {
                assert!(dir.path().join("clips").join(&filename).exists())
            }
            SaveOutcome::Duplicate { .. } => panic!("用户坚持了就不该再拦"),
        }
        assert_eq!(v.scan().unwrap().clips.len(), 2, "两篇都得在");
    }

    #[test]
    fn 删掉的可以重新剪() {
        // 删掉再剪同一篇,是"我后悔了想存回来",不是重复
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input("https://a.com/post", "标题", "正文"))
            .unwrap();
        v.trash(&saved.filename).unwrap();

        let again = input("https://a.com/post", "标题", "正文");
        assert!(matches!(
            v.save_checked(&again, false).unwrap(),
            SaveOutcome::Saved { .. }
        ));
    }
}
