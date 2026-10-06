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
/// 隔离区。**彻底删除只是搬到这儿,不是抹掉**
///
/// 产品承诺写着「误删了能一键撤销」,而原来这条承诺恰好在伤害最大的地方断了:
/// 移进回收站有撤销,「彻底删除」和「清空回收站」点下去就没了。
/// 加这一层是为了兑现那个承诺,不是为了给回收站套娃
pub const DELETED_DIR: &str = ".deleted";
/// 隔离区里的东西留多久。到期自动清,不然它只进不出,
/// 一年之后就是第二个剪藏库
pub const QUARANTINE_DAYS: i64 = 30;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("读写剪藏文件失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("文件名不合法,已拒绝: {0}")]
    UnsafeFilename(String),
    #[error("剪藏内容为空,已拒绝")]
    EmptyContent,
    /// 标题删空了。**不是"清空"而是"没了"**——列表里那一行会变成一片空白,
    /// 用户会以为软件坏了。宁可让这次改名失败,也不能在库里存一个空标题。
    #[error("标题不能是空的")]
    EmptyTitle,
    /// **这个文件在你编辑它的时候被别处改了。**
    ///
    /// 冲突时**什么都不写**。硬写下去丢的是用户刚敲的那一段,而那种丢失
    /// 是静默的:用户看着批注还在,过一会儿才发现正文也回到了旧版——
    /// 而他已经想不起来自己改过正文了
    #[error("这个文件在你编辑期间被别处改过,没敢写")]
    ChangedElsewhere,
    /// 一次批量里两项都没给。**这是调用方的错,不是用户的错**,所以硬失败。
    /// 早先这里返回「全部成功」,同一段注释里还写着"报全部成功是在骗人"——
    /// 注释说该拒绝,实现却照做了。现在谁来问都得到一句"你什么也没让我改"
    #[error("既没说改已读,也没说改归档,没动任何东西")]
    NothingToChange,
    #[error("剪藏不存在: {0}")]
    NotFound(String),
    #[error("已经有同名剪藏了,没敢放回去: {0}")]
    AlreadyExists(String),
    #[error("回收站里挤不下了,请自己清一清: {0}")]
    TrashFull(String),
    /// 标签给多了。**不是"多就截断"而是直接拒**:悄悄丢掉用户敲的第 31 个
    /// 标签,用户回头找不到,会以为软件把标签吃了。
    #[error("标签太多了,一篇最多 {0} 个")]
    TooManyTags(usize),
    /// 标签名不合法(空的、只有标点的、清洗之后和原来不是一个东西、
    /// 太长的)。**合并前就要拦下来**,不能扫到一半才发现,那时前面
    /// 几篇已经改了
    #[error("标签名不合法: {0}")]
    BadTag(String),
    /// 文件没有 Quire 的 frontmatter,改不了。**报出来而不是悄悄跳**——
    /// 合并标签是用户主动的批量操作,少改一篇他不会发现,回头发现时
    /// 标签已经是半新半旧的了
    #[error("读不出 frontmatter:{0}")]
    NoFrontmatter(String),
    /// 导入的目标就是剪藏库自己。**不是故障**——用户点"导入剪藏目录"
    /// 以为能去重,那是个陷阱,反复导库会滚成好几倍。得说清楚为什么不做。
    #[error("这就是剪藏库自己,不用导")]
    ImportSkippedSelf,
    /// 这一篇已经在库里了。**不是故障**——用户导进一个存过一堆旧文的
    /// 文件夹,里面有一半是重复的,那是正常情况。但理由要说给用户听。
    #[error("已经在库里了:{0}")]
    ImportSkippedDuplicate(String),
    /// 文件搬回去了但元数据解析失败。**文件已经回到库里了**——撤销是让用户
    /// 拿回东西的,不能因为读不出元数据就反悔把它留在回收站里。
    #[error("剪藏已放回,但读不出元数据: {0}({1})")]
    Unreadable(String, String),
    /// 内部锁在别的线程 panic 时被毒化。此时 vault 状态不可信,
    /// 宁可直接报错让用户重启,也不要拿着半可信状态继续读写用户的文件。
    #[error("内部状态异常,请重启 Quire")]
    Poisoned,
}

/// 错误穿过 Tauri 边界时带的**稳定代号**。
///
/// 后端只送代号和参数,不送现成的句子:界面是多语言的,后端说一句中文,
/// 英文用户看见的就是一堆看不懂的汉字。代号必须和界面词典里的键一一
/// 对上,少一条的话那个错误在界面上就只会甩一个 `vault.notFound`。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireError {
    pub code: String,
    /// 文案里的具名占位符。**用具名而不是按下标**:改文案时调一下参数
    /// 顺序,按下标取的就全错位了,而那种错不会报错,只是句子读不通。
    #[serde(default)]
    pub args: BTreeMap<String, String>,
}

impl WireError {
    pub fn new(code: &str) -> Self {
        Self {
            code: code.to_string(),
            args: BTreeMap::new(),
        }
    }

    pub fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.args.insert(key.to_string(), value.into());
        self
    }

    /// 内部出了岔子,原因未知。**只有真的没有更具体的代号时才用它**——
    /// 一律往这儿塞,界面就永远只能说"出了点岔子",等于没有错误提示。
    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new("app.internal").with("detail", detail)
    }
}

impl VaultError {
    /// 过界时用的代号 + 参数。**一个变体一条,漏写就编译不过**——
    /// 这正是要的效果:新增错误类型时,不可能忘了给它配文案。
    pub fn wire(&self) -> WireError {
        match self {
            Self::Io(e) => WireError::new("vault.io").with("detail", e.to_string()),
            Self::UnsafeFilename(name) => {
                WireError::new("vault.unsafeFilename").with("detail", name)
            }
            Self::EmptyContent => WireError::new("vault.emptyContent"),
            Self::EmptyTitle => WireError::new("vault.emptyTitle"),
            Self::ChangedElsewhere => WireError::new("vault.changedElsewhere"),
            Self::NothingToChange => WireError::new("vault.nothingToChange"),
            Self::NotFound(name) => WireError::new("vault.notFound").with("detail", name),
            Self::AlreadyExists(name) => {
                WireError::new("vault.alreadyExists").with("detail", name)
            }
            Self::TrashFull(name) => WireError::new("vault.trashFull").with("detail", name),
            Self::TooManyTags(limit) => {
                WireError::new("vault.tooManyTags").with("detail", limit.to_string())
            }
            Self::BadTag(reason) => WireError::new("vault.badTag").with("detail", reason),
            Self::NoFrontmatter(name) => {
                WireError::new("vault.noFrontmatter").with("detail", name)
            }
            Self::ImportSkippedSelf => WireError::new("vault.importSkippedSelf"),
            Self::ImportSkippedDuplicate(title) => WireError::new("vault.importSkippedDuplicate")
                .with("detail", title),
            Self::Unreadable(name, reason) => WireError::new("vault.unreadable")
                .with("name", name)
                .with("detail", reason),
            Self::Poisoned => WireError::new("vault.poisoned"),
        }
    }
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
    /// 用户手动标的"这个值得回头看"。和归档是两码事
    pub starred: bool,
    /// 读到哪儿了,0.0–1.0。没写过就是 0.0。
    pub progress: f32,
    pub tags: Vec<String>,
    /// 用户自己写的批注。空串就是没写,不进文件。
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreadableFile {
    pub filename: String,
    pub reason: String,
}

/// 一篇还有几张图指着外站。**指外站 = 图没下下来。**
///
/// Quire 一直说自己"数据在你手上",可导出的时候带图剪藏要是在别人机器上
/// 全是裂图,那句话就是空的。数出来是为了让用户知道自己有几篇是这样
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteImageClip {
    pub filename: String,
    pub title: String,
    pub count: usize,
}

/// 一篇有个能抓的地址,可正文短得不像全文。**多半是当初没抓到。**
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingFulltext {
    pub filename: String,
    pub title: String,
}

/// 库里有多少东西没做完。**空的表示什么都没欠**,界面上就不摆那个入口
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStatus {
    pub total_clips: usize,
    /// 还有图指着外站的
    pub remote_images: Vec<RemoteImageClip>,
    /// 可能没抓到全文的
    pub missing_fulltext: Vec<MissingFulltext>,
    /// 读不出的 `.md`
    pub unreadable: Vec<UnreadableFile>,
}

/// 数一篇里有几张图指着外站。
///
/// **只认 Markdown 的图片语法** `![...](http…)`。这不是为了严谨,
/// 是因为**只有这种图是 Quire 自己写进去的**——正文里别人手写的
/// 外链不该被算成"Quire 没存好",而链接(不带 `!`)压根不是图
///
/// **`!` 认的是 alt 文本** `[...]` **前面那一位。** `![图](u)` 和 `[链接](u)`
/// 在 `](` 之后长得一模一样,区别全在前面。认错了就会把正文里的每一个超链接
/// 都数成"没存好的图",而这一栏的全部意义就是可信
pub fn remote_image_count(body: &str) -> usize {
    let mut n = 0usize;
    let mut from = 0usize;
    while let Some(rel) = body[from..].find("](") {
        let close = from + rel; // ']' 的字节下标
        let head = &body[..close];
        // 往回找配对的那个 '['。**alt 文本里带方括号是极少数**,那种情况下
        // 可能数错一个——但比把正文里每一个超链接都算成"没存好的图"好得多,
        // 后者会让这一栏彻底不可信
        let is_image = head
            .rfind('[')
            .is_some_and(|lb| lb > 0 && head.as_bytes()[lb - 1] == b'!');
        if is_image {
            let target = body[close + 2..].trim_start_matches(['<', ' ', '\t']);
            if target.starts_with("http://") || target.starts_with("https://") {
                n += 1;
            }
        }
        from = close + 2;
    }
    n
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
    /// 读这一篇时**磁盘上那份**的指纹。改批注、改标题、补全文的时候带回来,
    /// 后端写之前会比一次
    ///
    /// **为什么要指纹,不用 mtime。** mtime 有 1 秒或者 2 秒的粒度(取决于
    /// 文件系统),而"你在 Obsidian 里改了、我这边失焦保存"可能就隔几百毫秒——
    /// 那一次 mtime 压根没变,冲突就漏了。指纹是内容的,改一个字节就变
    #[serde(default)]
    pub stamp: String,
}

/// 内容指纹。**只算用户手写的那几项,不算阅读状态。**
///
/// **为什么不直接哈希整份文件。** `progress` 滚动一下写一次、`read` 随手
/// 就标,而这些恰恰是 Quire **自己**在写。要是算进去,用户开着详情页边读
/// 边写批注,中间滚动了两下,下一次自动存批注就撞上指纹不符——弹一句
/// "这个文件在你编辑期间被别处改过",而外面根本没人碰过。天天喊狼来了
/// 的守卫,用户第三次就学会直接忽略它,那时它挡住的和没装一样
///
/// **算哪几项。** `title` / `note` / `tags` / 正文——用户手敲、敲了就
/// 心疼的东西。其余(id、地址、剪藏时间、封面、已读、归档、收藏、进度)
/// 要么是机器生成的,要么是读到哪里一类的记账,被别处改一下不构成冲突
///
/// **FNV-1a 64,手写而不是 `DefaultHasher`**——后者不保证跨版本稳定,
/// 哪天 Rust 换了算法,用户编辑器里存着的旧指纹就集体变成"不认识"，
/// 于是每一次保存都被判成冲突,而用户除了挨个放弃保存没别的办法
pub fn content_stamp(text: &str) -> String {
    // 读不出 frontmatter 的文件(用户自己丢进来的手写笔记)就整份算。
    // 这种文件本来也不走带指纹的写路径,当个兜底
    let Some((block, body)) = frontmatter::split(text) else {
        return hash_stamp(&[("body", text)]);
    };
    let fm = Frontmatter::parse(block);
    hash_stamp(&[
        ("title", &fm.title),
        ("note", &fm.note),
        ("tags", &fm.tags.join("\u{1}")),
        ("body", body),
    ])
}

/// 把若干段拼起来算一个指纹。**段与段之间垫一个不会被用户敲出来的分隔符**——
/// 不然「ab」+「c」和「a」+「bc」会撞成同一个值,而那意味着改一个字段
/// 漏判成没改
fn hash_stamp(parts: &[(&str, &str)]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            h ^= u64::from(*byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (key, value) in parts {
        eat(key.as_bytes());
        eat(&[0x1]);
        eat(value.as_bytes());
        eat(&[0x1]);
    }
    format!("{h:016x}")
}

/// 写盘前的冲突检查。
///
/// **返回 `Err` 的一方什么也不写。** 这一条是这里全部的意义:冲突了还照写,
/// 丢的就是用户在 Obsidian 里刚敲的那一段
fn check_stamp(text: &str, expected: Option<&str>) -> Result<(), VaultError> {
    let Some(expected) = expected else { return Ok(()) };
    if content_stamp(text) == expected {
        return Ok(());
    }
    Err(VaultError::ChangedElsewhere)
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

/// 回收站里的一条。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashItem {
    pub filename: String,
    /// 读不出 frontmatter 时是 `None`。**照样要显示、照样能彻底删**,
    /// 否则用户既看不见它,也清不掉它。
    pub summary: Option<ClipSummary>,
    /// 放出来是为了让用户判断"这篇还有没有必要留着",顺便暴露一个事实:
    /// 回收站不是免费的,它是占着磁盘的。
    ///
    /// **拿不到大小就是 `None`,不是 0。** "彻底删除"是全软件唯一不可撤销的
    /// 操作,删之前给的体积承诺得准:发 0 过去,界面上写的就是"删掉能省 0 B",
    /// 用户会以为自己算错了。拿不到就别给数字,前端会换成一句不含大小的说法
    pub size_bytes: Option<u64>,
    /// **它已经在隔离区里了吗。** 界面上要分开说:回收站里的还能一键翻回,
    /// 隔离区里的要过两道手。用户分不清这两者,「彻底删除」就还是闭眼签字
    #[serde(default)]
    pub quarantined: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashListing {
    pub items: Vec<TrashItem>,
    /// **保留期下发到前端,不在界面里硬编码。** 界面上要写"30 天后自动清理",
    /// 那个 30 就是这个值。下次改保留期只改 [`QUARANTINE_DAYS`] 一处;
    /// 前端自己写死一个 30,改常量那天界面就开始撒谎
    pub quarantine_days: i64,
}

/// 一批操作的结果。**批量最容易出的事就是"悄悄少做了一半"**:一次改 30 篇,
/// 中间有一篇被占用、有一篇文件名不合法,用户看到的还是"操作成功"。
/// 所以成败必须分开报,失败的连原因一起带回来。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchReport {
    pub succeeded: Vec<String>,
    pub failed: Vec<PurgeFailure>,
}

/// 标签改名的结果。**改了几篇 + 哪几篇没改成**
///
/// 单独报 `changed` 而不是沿用 `BatchReport` 的成功列表:合并标签时
/// 绝大多数文件是**正常的、只是没带这个标签**,全列进「成功」会把
/// 报告撑成几百行,真正需要用户知道的反而被淹掉
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagRenameReport {
    pub changed: usize,
    pub failed: Vec<PurgeFailure>,
}

/// 撤销一批删除的结果:成败报告 + 放回来的剪藏摘要。
/// 分开两个字段是因为两边的**文件名不是一回事**——`report` 里是用户
/// 点撤销时传进来的名字,`clips` 里是落回剪藏库之后、已经能直接进列表的摘要。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoredBatch {
    pub report: BatchReport,
    pub clips: Vec<ClipSummary>,
}

/// 导入的结果。两边的文件名**不是一回事**,所以分开两个字段:
/// `report` 里是用户源文件夹里的原名(报错要指得准),
/// `imported` 里是落进剪藏库之后的新文件名(图片本地化按它找文件)。
/// 标签栏上的一项:标签本身 + 有几篇用它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub tag: String,
    pub count: usize,
}

/// 一篇剪藏最多几个标签。列表上一行放不下十几个,而真正常用的标签
/// 一个人也超不过十个——超了基本是在乱敲。
pub const MAX_TAGS: usize = 20;

/// 一个标签最多几个字。**标签栏就那么宽**,几十个字一个标签会把整栏挤爆,
/// 而用户是从粘贴一段话进来的,他不会觉得自己敲了"一个标签"
pub const MAX_TAG_LEN: usize = 40;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub report: BatchReport,
    pub imported: Vec<String>,
    /// 压根没读进去的**目录**。文件名读不出来的话 `report.failed` 还能收一条,
    /// 可目录读不出来是整批文件人间蒸发——原来连一个数字都不给用户,
    /// 弹一句"导入完成 240 篇",他以为全导完了,几天后才发现少了 12 篇。
    /// 报告里没有"没找到文件"这个概念,是他唯一能查到的线索
    pub unreadable_dirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeFailure {
    pub filename: String,
    /// 失败理由。**送代号不送句子**,理由和上面一样:这句话最终要显示
    /// 给用户看,而用户用的是哪种语言,不归写盘的文件名管。
    pub reason: WireError,
}

pub struct Vault {
    root: PathBuf,
    /// Quire 自己写盘的时刻。**文件监控靠它把自家的写盘排除掉**——
    /// 不排除的话,每一次剪藏、每一次标已读,都紧跟着一次全量重扫,
    /// 用户会看到列表刚更新完又跳一下
    ///
    /// 放在 `Vault` 里而不是在每个命令里各插一行:**写盘的是它,
    /// 不是命令**。往命令里插的话,漏一个就是一个只在特定操作下
    /// 才出现的抖动,而那种 bug 极难复现
    guard: crate::watcher::WriteGuard,
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
        Self {
            root: root.into(),
            guard: crate::watcher::WriteGuard::new(),
        }
    }

    /// 记一笔"Quire 刚写过盘"。**所有会写盘的方法开头都调它**——
    /// 文件监控靠它把自家的写盘排除掉,漏一处就是一个只在那个操作下
    /// 才出现的抖动(剪藏之后列表莫名重扫)
    pub fn note_write(&self) {
        self.guard.note_write();
    }

    /// 文件监控线程用的那一份。**和写盘那边是同一个对象**——
    /// 复制一份的话两边各记各的,排除就完全失效了
    pub fn write_guard(&self) -> crate::watcher::WriteGuard {
        self.guard.clone()
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
        self.note_write();
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
            note: String::new(),
            read: false,
            archived: false,
            starred: false,
            progress: 0.0,
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
        self.note_write();
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
        starred: Option<bool>,
    ) -> Result<ClipSummary, VaultError> {
        self.note_write();
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
        let before = (fm.read, fm.archived, fm.starred);
        if let Some(v) = read {
            fm.read = v;
        }
        if let Some(v) = archived {
            fm.archived = v;
        }
        if let Some(v) = starred {
            fm.starred = v;
        }

        // **值没变就不写。** 和 `set_title` / `set_note` 同一个闸:批量那条路
        // 会连着发几十次请求,其中"把已经标过的再标一遍"太常见了。
        // 没变还去 `write_atomic` 换一次 mtime,不只是浪费磁盘写入——
        // 用户的同步盘和备份软件都会跟着醒一趟,空转一圈
        if (fm.read, fm.archived, fm.starred) == before {
            return Ok(summary_from(fm, filename.to_string()));
        }

        // 分隔符后**只跟一个换行**:split 切出来的 body 自带原来那个空行,
        // 再补一个就等于每次改标志给正文加一行,文件会越滚越胖。
        let mut out = fm.render().replace('\n', newline);
        out = format!("---{newline}{out}---{newline}{body}");
        write_atomic(&path, out.as_bytes())?;

        Ok(summary_from(fm, filename.to_string()))
    }

    /// 改标题。
    ///
    /// 走的是和 `set_flags` 完全一样的路子:只重新序列化 frontmatter,
    /// 正文原样拼回去,用户的字段和换行风格都不动。
    ///
    /// 两条和 `set_flags` 不同的规矩:
    ///
    /// **一是空标题直接拒。** 标签可以没有,标题不行——列表里那一行
    /// 靠标题撑着,空了就是一排空白,用户会以为软件坏了。
    ///
    /// **二是"值没变就不写"。** 标题是在输入框里边打边存的,每敲一下
    /// 都可能来一次。没变还去 `write_atomic` 换 mtime,那不只是浪费
    /// 磁盘写入,还会把自己的文件监控吵醒,空转一圈。
    pub fn set_title(
        &self,
        filename: &str,
        title: &str,
        stamp: Option<&str>,
    ) -> Result<ClipSummary, VaultError> {
        self.note_write();
        let title = title.trim();
        if title.is_empty() {
            return Err(VaultError::EmptyTitle);
        }
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        let path = self.clips_dir().join(filename);
        let original =
            fs::read_to_string(&path).map_err(|_| VaultError::NotFound(filename.to_string()))?;
        check_stamp(&original, stamp)?;
        let (block, body) = frontmatter::split(&original)
            .ok_or_else(|| VaultError::NotFound(filename.to_string()))?;

        let newline = if block.contains("\r\n") { "\r\n" } else { "\n" };

        let mut fm = Frontmatter::parse(block);
        if fm.title == title {
            return Ok(summary_from(fm, filename.to_string()));
        }
        fm.title = title.to_string();

        let mut out = fm.render().replace('\n', newline);
        out = format!("---{newline}{out}---{newline}{body}");
        write_atomic(&path, out.as_bytes())?;

        Ok(summary_from(fm, filename.to_string()))
    }

    /// 记读到哪儿了。
    ///
    /// 走的是和 `set_flags` 完全一样的路子:只重新序列化 frontmatter,
    /// 正文原样拼回去,用户的字段和换行风格都不动。
    ///
    /// **闸是"值没变就不写",不是"值是 0 就不写"。** 进度是滚动条给的,
    /// 每抖一下都来一次,差不到一个百分点直接返回。用户什么都不干的时候
    /// 文件的 mtime 不该被刷成一片,那既是磁盘写入也是无意义的元数据变更
    /// (还会把文件监控自己吵醒)。
    ///
    /// 反过来,滚回顶部(0)是**真的位置变化**,得记下来:用户重新打开一篇
    /// 读了一半的文章又从头看,那"读到哪儿"的答案就该是开头。
    pub fn set_progress(&self, filename: &str, progress: f32) -> Result<ClipSummary, VaultError> {
        self.note_write();
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        let clamped = if progress.is_finite() {
            progress.clamp(0.0, 1.0)
        } else {
            0.0
        };

        let path = self.clips_dir().join(filename);
        let original =
            fs::read_to_string(&path).map_err(|_| VaultError::NotFound(filename.to_string()))?;
        let (block, body) = frontmatter::split(&original)
            .ok_or_else(|| VaultError::NotFound(filename.to_string()))?;
        let newline = if block.contains("\r\n") { "\r\n" } else { "\n" };
        let mut fm = Frontmatter::parse(block);
        if (fm.progress - clamped).abs() < 0.01 {
            return Ok(summary_from(fm, filename.to_string()));
        }
        fm.progress = clamped;

        let mut out = fm.render().replace('\n', newline);
        out = format!("---{newline}{out}---{newline}{body}");
        write_atomic(&path, out.as_bytes())?;
        Ok(summary_from(fm, filename.to_string()))
    }

    /// 改一篇的标签。走的是和 `set_flags` 完全一样的路子:只重新序列化
    /// frontmatter,正文原样拼回去,用户的字段和换行风格都不动。
    ///
    /// **入库前先洗一遍标签。** 标签是用户随手敲的,敲个引号进去
    /// `tags: ["a"b"]`,整个 frontmatter 当场解析不出来,这篇剪藏从此打不开。
    /// 与其指望用户不敲引号,不如在门口挡住。
    pub fn set_tags(&self, filename: &str, tags: &[String]) -> Result<ClipSummary, VaultError> {
        self.note_write();
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        let mut cleaned = clean_tags(tags);
        if cleaned.len() > MAX_TAGS {
            return Err(VaultError::TooManyTags(MAX_TAGS));
        }
        // 长度也得拦。**两条入口规则必须一样**——合并走一条、改标签走
        // 另一条,一个允许 200 字一个不允许,用户会觉得这软件有毛病
        cleaned = cleaned
            .iter()
            .map(|t| validate_tag(t))
            .collect::<Result<Vec<_>, _>>()?;

        let path = self.clips_dir().join(filename);
        let original =
            fs::read_to_string(&path).map_err(|_| VaultError::NotFound(filename.to_string()))?;
        let (block, body) = frontmatter::split(&original)
            .ok_or_else(|| VaultError::NotFound(filename.to_string()))?;
        let newline = if block.contains("\r\n") { "\r\n" } else { "\n" };

        let mut fm = Frontmatter::parse(block);
        // 顺序不同算不同:用户在界面上把「待读」拖到「重要」前面,
        // 那是他自己排的序,不该被悄悄按字母重排
        if fm.tags == cleaned {
            return Ok(summary_from(fm, filename.to_string()));
        }
        fm.tags = cleaned;

        let mut out = fm.render().replace('\n', newline);
        out = format!("---{newline}{out}---{newline}{body}");
        write_atomic(&path, out.as_bytes())?;
        Ok(summary_from(fm, filename.to_string()))
    }

    /// 标签改名 / 合并。`from` 换成 `to`,`to` 已经有了就是合并(取并集)
    ///
    /// **回收站里的也一起改。** 只改剪藏库的话,用户合并完标签,
    /// 哪天从回收站放回一篇,旧标签又回来了——他会觉得"合了等于没合"
    ///
    /// **标签顺序原样保留。** 顺序是用户自己排的,合并时重排等于
    /// 替他做了个没人要求的决定
    pub fn rename_tag(&self, from: &str, to: &str) -> Result<TagRenameReport, VaultError> {
        self.note_write();
        let from = from.trim();
        let to = to.trim();
        if from.is_empty() {
            return Err(VaultError::BadTag("原标签不能是空的".to_string()));
        }
        // 新名字过一遍和 `set_tags` 一样的闸。**先校验再动手**,
        // 免得扫到一半才发现名字不合法,前面几篇已经改了
        let to_checked = validate_tag(to)?;
        let to = to_checked.as_str();

        let mut report = TagRenameReport::default();
        for dir in [self.clips_dir(), self.trash_dir()] {
            if !dir.exists() {
                continue;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
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
                match self.rename_tag_in(&path, from, to) {
                    Ok(true) => report.changed += 1,
                    // 没带这个标签:一个字节都不该动
                    Ok(false) => {}
                    Err(e) => report.failed.push(PurgeFailure {
                        filename,
                        reason: e.wire(),
                    }),
                }
            }
        }
        Ok(report)
    }

    /// 只改 frontmatter 里的标签,正文原样拼回去。**返回有没有真的改**
    fn rename_tag_in(&self, path: &Path, from: &str, to: &str) -> Result<bool, VaultError> {
        let text = fs::read_to_string(path)?;
        let Some((block, body)) = frontmatter::split(&text) else {
            return Err(VaultError::NoFrontmatter(path.display().to_string()));
        };
        let newline = if block.contains("\r\n") { "\r\n" } else { "\n" };
        let mut fm = Frontmatter::parse(block);
        let Some(at) = fm.tags.iter().position(|t| t == from) else {
            return Ok(false);
        };
        // 位置原位替换,顺序不动。目标已经有了就删掉这个位置——
        // 同一个标签出现两次,标签栏上会显示成「重要 7」而实际只有 4 篇
        if fm.tags.iter().any(|t| t == to) {
            fm.tags.remove(at);
        } else {
            fm.tags[at] = to.to_string();
        }
        if fm.tags.len() > MAX_TAGS {
            return Err(VaultError::TooManyTags(MAX_TAGS));
        }
        let mut out = fm.render().replace('\n', newline);
        out = format!("---{newline}{out}---{newline}{body}");
        write_atomic(path, out.as_bytes())?;
        Ok(true)
    }

    /// 标签栏的数据:每个标签 + 有几篇在用,按篇数倒序。
    ///
    /// **只算没归档的。** 标签栏是给「还打算看的那些」用的,一堆归档了的
    /// 老标签混在里面,用户点着点着就以为标签乱了。同名再算一遍会变成
    /// 「重要 7」而实际只有 3 篇还活着,那比不显示更糟。
    pub fn tag_index(&self) -> Result<Vec<TagCount>, VaultError> {
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for clip in self.scan()?.clips {
            if clip.archived {
                continue;
            }
            for tag in clip.tags {
                *counts.entry(tag).or_insert(0) += 1;
            }
        }
        let mut out: Vec<TagCount> = counts
            .into_iter()
            .map(|(tag, count)| TagCount { tag, count })
            .collect();
        // 篇数多的在前;一样多时按标签名排,免得每次打开顺序都在跳
        out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
        Ok(out)
    }

    /// 写批注。
    ///
    /// 走的是和 `set_title` / `set_flags` 完全一样的路子:只重新序列化
    /// frontmatter,正文原样拼回去,用户的字段和换行风格都不动。
    ///
    /// **批注允许清空。** 标题空了列表上是一行空白,所以标题得拒;
    /// 批注是可选的,用户就是想能删掉。
    ///
    /// **"值没变就不写"。** 批注是在输入框里边打边存的,每一下都可能
    /// 来一次调用。没改还去写,不只是浪费,还会把自己的文件监控吵醒。
    ///
    /// `stamp` 是读这一篇时那份磁盘内容的指纹。**对不上就什么都不写**——
    /// 典型场景是:你打开详情页写了半句批注(还没失焦、还没存),这中间在
    /// Obsidian 里改了正文,回来接着写、失焦保存。不查这一下的话,
    /// 丢的不只是批注,**正文也会一起被写回成旧的那份**
    pub fn set_note(
        &self,
        filename: &str,
        note: &str,
        stamp: Option<&str>,
    ) -> Result<ClipSummary, VaultError> {
        self.note_write();
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        // 只砍首尾空白,中间的换行是用户自己排的版,一个字都不许动
        let note = note.trim();
        let path = self.clips_dir().join(filename);
        let original =
            fs::read_to_string(&path).map_err(|_| VaultError::NotFound(filename.to_string()))?;
        check_stamp(&original, stamp)?;
        let (block, body) = frontmatter::split(&original)
            .ok_or_else(|| VaultError::NotFound(filename.to_string()))?;

        let newline = if block.contains("\r\n") { "\r\n" } else { "\n" };

        let mut fm = Frontmatter::parse(block);
        if fm.note == note {
            return Ok(summary_from(fm, filename.to_string()));
        }
        fm.note = note.to_string();

        let mut out = fm.render().replace('\n', newline);
        out = format!("---{newline}{out}---{newline}{body}");
        write_atomic(&path, out.as_bytes())?;

        Ok(summary_from(fm, filename.to_string()))
    }

    /// **只换正文,frontmatter 一个字都不动。**
    ///
    /// 这条路的用途很窄:剪藏那一刻网页抓不到,只存了用户复制的那一小段,
    /// 几周后他想补全。现在他手上有地址了,重试一次就能把整篇补进去——
    /// 而**补的时候不能顺手改掉他的标签、批注和阅读进度**。那些是他在这
    /// 篇上花过时间的证据,一个"重试"把它们清掉,那不是帮忙
    ///
    /// **正文更长才写。** 抓回来的万一比原来的还短(页面改了、
    /// 抽出来只剩个壳),拿它覆盖掉用户已经有的内容是净损失。那就原样不动
    pub fn set_body(
        &self,
        filename: &str,
        markdown: &str,
        stamp: Option<&str>,
    ) -> Result<ClipSummary, VaultError> {
        self.note_write();
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        // 和批注一个规矩:**只砍首尾空白,中间的换行一个字不动**
        let body_new = markdown.trim();
        if body_new.is_empty() {
            return Err(VaultError::EmptyContent);
        }
        let path = self.clips_dir().join(filename);
        let original =
            fs::read_to_string(&path).map_err(|_| VaultError::NotFound(filename.to_string()))?;
        check_stamp(&original, stamp)?;
        let (block, body) = frontmatter::split(&original)
            .ok_or_else(|| VaultError::NotFound(filename.to_string()))?;

        // 比的是**字数不是字节数**。用户从别的工具复制来的正文可能带 CRLF,
        // 按字节比会出现"其实一样长,非说变短了"的白跑一趟
        if body_new.chars().count() <= body.trim().chars().count() {
            return Ok(summary_from(Frontmatter::parse(block), filename.to_string()));
        }

        let newline = if block.contains("\r\n") { "\r\n" } else { "\n" };
        let fm = Frontmatter::parse(block);
        let mut out = fm.render().replace('\n', newline);
        out = format!("---{newline}{out}---{newline}{body_new}");
        write_atomic(&path, out.as_bytes())?;

        Ok(summary_from(fm, filename.to_string()))
    }

    /// **Quire 干得怎么样,一次问清楚。**
    ///
    /// 图没下下来、抓取失败、文件读不出——这些过去都是一次性的提示,
    /// 弹过就没了。用户永远不知道"我这一周有多少东西没存全",而
    /// 「数据在你手上」这句话,图要是没下下来还一声不吭,就是在骗人
    ///
    /// **数的是当下的文件,不是记下来的事件。** 记"当时失败了几张"的话,
    /// 用户后来自己把图补上、或者换了剪藏库,那条记录就过期了——
    /// 而一份过期的账比没有账更坏:它会让用户以为还有图没处理,或者反过来
    pub fn library_status(&self) -> Result<LibraryStatus, VaultError> {
        let dir = self.clips_dir();
        let mut status = LibraryStatus {
            total_clips: 0,
            ..Default::default()
        };
        if !dir.exists() {
            return Ok(status);
        }
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
            let Ok(text) = fs::read_to_string(&path) else {
                status
                    .unreadable
                    .push(UnreadableFile { filename, reason: "读不出这个文件".into() });
                continue;
            };
            let Some((block, body)) = frontmatter::split(&text) else {
                status.unreadable.push(UnreadableFile {
                    filename,
                    reason: "没有 Quire 的 frontmatter".into(),
                });
                continue;
            };
            let fm = Frontmatter::parse(block);
            if fm.id.is_empty() {
                status.unreadable.push(UnreadableFile {
                    filename,
                    reason: "frontmatter 里没有 id".into(),
                });
                continue;
            }
            status.total_clips += 1;

            let remote = remote_image_count(body);
            if remote > 0 {
                status.remote_images.push(RemoteImageClip {
                    filename: filename.clone(),
                    title: fm.title.clone(),
                    count: remote,
                });
            }
            // 和「补全全文」那个按钮**同一条判据**。两处各写一份的话,
            // 迟早有一处说"这篇能补"而另一处说"这篇没问题"
            let url = fm.url.trim();
            if crate::fetch::looks_fetchable(url)
                && body.trim().chars().count() < crate::fetch::MIN_CLIP_CHARS_FOR_FETCH
            {
                status.missing_fulltext.push(MissingFulltext {
                    filename: filename.clone(),
                    title: fm.title.clone(),
                });
            }
        }
        // 篇数多的排前面。**先看到最严重的那几篇**,而不是随机顺序
        status.remote_images.sort_by_key(|c| std::cmp::Reverse(c.count));
        status.unreadable.sort_by(|a, b| a.filename.cmp(&b.filename));
        status.missing_fulltext.sort_by(|a, b| a.title.cmp(&b.title));
        Ok(status)
    }

    /// 一次给多篇打标签。
    ///
    /// 挨个走 `set_tags` 的同一条路,不先在内存里算一遍再统一写盘:
    /// 每个文件的内容都不一样(用户自己加过字段、换行风格也不一样),
    /// 只有走同一条才能保证"不许弄坏用户文件"那几条规矩在批量下照样成立。
    ///
    /// **标签是替换,不是追加。** 用户挑了一堆想统一打上「重要」,
    /// 变成追加的话他之前那堆五花八门的标签还留着,标签栏会越来越脏。
    ///
    /// 标签数超了直接整批拒绝,不截断:悄悄丢掉用户敲的那几个,
    /// 他回头找不到,会以为软件把标签吃了。**一个标签都没改**就先返回。
    pub fn set_tags_batch(
        &self,
        filenames: &[String],
        tags: &[String],
    ) -> Result<BatchReport, VaultError> {
        self.note_write();
        let cleaned = clean_tags(tags);
        if cleaned.len() > MAX_TAGS {
            return Err(VaultError::TooManyTags(MAX_TAGS));
        }
        let mut report = BatchReport::default();
        for filename in filenames {
            self.record(&mut report, filename, || {
                self.set_tags(filename, &cleaned).map(|_| ())
            });
        }
        Ok(report)
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
        let items = self.collect_export(None)?;
        Ok(Self::render_export(&items))
    }

    /// 导出**指定的**几篇。
    ///
    /// 名单外的**读都不读**,不是读完了再扔:导出两百篇时白读一百九十九篇,
    /// 是白花的时间。名单里的读不出来就当没这一篇,不报错也不提——那个文件
    /// 此刻可能正被同步软件锁着,为它让整个导出失败,等于因为一个文件
    /// 拿不回其余全部,而那个文件他本来就有,随时能再导一次
    pub fn export_selected(&self, filenames: &[String]) -> Result<String, VaultError> {
        let items = self.collect_export(Some(filenames))?;
        Ok(Self::render_export(&items))
    }

    /// 导出成一个**文件夹**:里面是拼好的 Markdown,加上 `assets/`。
    ///
    /// 为什么非得是文件夹:剪藏时图片下到本地 `assets/<id>/3.png`,正文里写的
    /// 就是这个相对路径。只导出单个 `.md` 的话,用户把它发给别人、在手机上
    /// 打开、在 Obsidian 里看——**一张图都裂**。而带图剪藏恰恰是最需要
    /// "我的数据是我的"这句话成立的那一类,导出时全丢图,这条承诺就是假的
    ///
    /// **目录里没有的、用户自己塞进去的文件一个都不碰。** 我们只往里写
    /// 自己的东西,不删别人的:用户可能挑了个现成的文件夹当导出位置,
    /// 里面已经有他的笔记了
    pub fn export_folder(
        &self,
        target: &Path,
        filenames: Option<&[String]>,
    ) -> Result<ExportFolder, VaultError> {
        let items = self.collect_export(filenames)?;
        fs::create_dir_all(target)?;

        let name = match filenames {
            Some(list) if list.len() == 1 => "剪藏.md".to_string(),
            Some(list) => format!("剪藏-{}.md", list.len()),
            None => "剪藏.md".to_string(),
        };
        fs::write(target.join(&name), Self::render_export(&items))?;

        // 图片按 **id** 挑目录:`assets/<clip_id>/`。正文里引用的就是
        // `assets/<id>/3.png`,而 id 就写在那篇的 frontmatter 里,两边对得上
        let assets_target = target.join(crate::assets::ASSETS_DIR);
        let (mut copied, mut missing) = (0usize, 0usize);
        for (_, fm, _) in &items {
            if fm.id.is_empty() {
                continue;
            }
            let from = self.assets_dir().join(&fm.id);
            // **只导这篇确实有的。** 没下成功的图(下失败的那些保持原样)
            // 在这里就是不存在的目录,跳过而不是报错——导出的 Markdown 里
            // 那个地址还指着原站,能用
            if !from.is_dir() {
                continue;
            }
            let to = assets_target.join(&fm.id);
            // 一篇拷不动不该让整份导出泡汤。那篇的图会缺,但正文和其余的图
            // 都还在——比什么都不给强,所以数一下报给用户
            match copy_dir(&from, &to) {
                Ok(()) => copied += count_files(&to),
                Err(_) => missing += 1,
            }
        }
        Ok(ExportFolder {
            markdown: name,
            images: copied,
            failed_images: missing,
        })
    }

    /// 收集要导出的那几篇。`filter` 是 `None` 表示全要,`Some` 表示只留名单里的
    fn collect_export(
        &self,
        filter: Option<&[String]>,
    ) -> Result<Vec<(String, Frontmatter, String)>, VaultError> {
        let dir = self.clips_dir();
        if !dir.exists() {
            return Ok(Vec::new());
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
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Some(wanted) = filter {
                if !wanted.iter().any(|w| w == name) {
                    continue;
                }
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
        Ok(items)
    }

    /// 收集好的那几篇拼成一份导出。**排序在这里做**,不在收集里——
    /// 两个导出入口共用这一段,免得其中一个哪天忘了排,导出的顺序每次都在跳
    fn render_export(items: &[(String, Frontmatter, String)]) -> String {
        if items.is_empty() {
            return empty_export();
        }
        // 和列表一致:新剪的在前。id 自带时间序,同秒内也不会乱。
        let mut sorted = items.to_vec();
        sorted.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));

        let mut out = String::new();
        out.push_str("# Quire 剪藏导出\n\n");
        out.push_str(&format!("共 {} 篇,由 Quire 导出。\n\n", sorted.len()));
        out.push_str("| 剪藏于 | 标题 | 原文 |\n| --- | --- | --- |\n");
        for (clipped_at, fm, _) in &sorted {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                clipped_at,
                escape_cell(&fm.title),
                fm.url
            ));
        }
        out.push_str("\n---\n");

        for (_, fm, body) in &sorted {
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
            // 标签和批注**一起带上**。用户挑出几篇导出去,多半是为了
            // 发给别人或者换工具读,这两样是"这批东西是我的"的证据,
            // 只搬正文就等于把他整理过的痕迹扔了
            if !fm.tags.is_empty() {
                meta.push(format!("- 标签 {}", fm.tags.join("、")));
            }
            if !fm.note.trim().is_empty() {
                meta.push(format!("- 批注 {}", fm.note.trim().replace('\n', " ")));
            }
            out.push_str(&meta.join("\n"));
            out.push_str("\n\n");
            out.push_str(body);
            out.push('\n');
        }
        out
    }

    /// 回收站。**点错了不该找不回来**,这是剪藏工具里唯一一个能把用户东西
    /// 弄没的操作,所以它不删文件,只搬到 `clips/.trash/`。用户后悔了可以
    /// 撤销,没撤销也还能自己去剪藏目录里把文件捞出来。
    /// 隔离区。**回收站里那层,再往里一层**
    pub fn deleted_dir(&self) -> PathBuf {
        self.clips_dir().join(DELETED_DIR)
    }

    /// 回收站和隔离区里都找一遍。**顺序有讲究:先回收站**——
    /// 同一个文件名两处都有的极少见,但真出现了的话,界面上的那个
    /// 才是用户实际操作的那个
    fn find_in_trash(&self, filename: &str) -> Option<PathBuf> {
        let in_trash = self.trash_dir().join(filename);
        if in_trash.is_file() {
            return Some(in_trash);
        }
        let in_quarantine = self.deleted_dir().join(filename);
        in_quarantine.is_file().then_some(in_quarantine)
    }

    pub fn trash_dir(&self) -> PathBuf {
        self.clips_dir().join(TRASH_DIR)
    }

    /// 把一篇剪藏移进回收站。文件**搬走**而不是复制,搬完原位置就没了。
    pub fn trash(&self, filename: &str) -> Result<(), VaultError> {
        self.note_write();
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
        self.note_write();
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        // **回收站和隔离区都认。** 只找回收站的话,「彻底删除之后再翻回来」
        // 这条路是断的,而它恰恰是用户按了「彻底删除」之后最想做的事
        let from = self
            .find_in_trash(filename)
            .ok_or_else(|| VaultError::NotFound(filename.to_string()))?;
        let to = self.clips_dir().join(filename);
        // 原位已经有同名的(用户手工放回来过),那就别动它,报冲突
        if to.exists() {
            return Err(VaultError::AlreadyExists(filename.to_string()));
        }
        let id = self.id_of_in(&from);
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

    /// 彻底删掉一篇的图片目录。删完之后顺手把空壳收掉,
    /// 否则 `assets/` 底下会积一堆空目录,用户打开剪藏目录一看还以为漏了东西。
    fn remove_assets(&self, id: &str) -> Result<(), VaultError> {
        let root = self.trash_dir().join(crate::assets::ASSETS_DIR).join(id);
        if root.is_dir() {
            fs::remove_dir_all(&root)?;
        }
        // 可能是这一篇删完之后这个 id 桶空了;也可能是之前就有空壳,顺手收掉
        for parent in [
            self.trash_dir().join(crate::assets::ASSETS_DIR),
            self.trash_dir(),
        ] {
            if parent.is_dir() && fs::read_dir(&parent)?.next().is_none() {
                let _ = fs::remove_dir(&parent);
            }
        }
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
    /// 回收站里剩下什么。按剪藏时间倒序,和主列表一个次序。
    ///
    /// **读不出元数据的文件照样列出来**,`summary` 留 `None`。那正是最该被
    /// 看见、也最该能被彻底删掉的一批——只显示"解析得动的",用户会以为
    /// 回收站已经清空了,而实际上有东西躺在那儿既看不见也删不掉。
    /// 回收站**和隔离区**一起列。`quarantined` 说清哪一篇已经进隔离区
    pub fn scan_trash(&self) -> Result<TrashListing, VaultError> {
        let mut items = Vec::new();
        for (dir, quarantined) in [(self.trash_dir(), false), (self.deleted_dir(), true)] {
            if !dir.exists() {
                continue;
            }
            for entry in fs::read_dir(&dir)?.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                // 文件可能在扫描过程中被用户自己从文件管理器里删了。拿不到
                // 大小就报"没有这个数",别报 0——0 会被当成"这篇不占地方"
                let size_bytes = entry.metadata().map(|m| m.len()).ok();
                items.push(TrashItem {
                    filename: filename.to_string(),
                    summary: read_summary(&path, filename).ok(),
                    size_bytes,
                    quarantined,
                });
            }
        }
        items.sort_by(|a, b| match (&a.summary, &b.summary) {
            (Some(x), Some(y)) => y.id.cmp(&x.id),
            // 有元数据的排在前面,没元数据的按文件名排,免得每回刷新顺序都在跳
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.filename.cmp(&b.filename),
        });
        Ok(TrashListing {
            items,
            quarantine_days: QUARANTINE_DAYS,
        })
    }

    /// 导入一个文件夹里的 Markdown。
    ///
    /// **只读源目录,一个字节都不写回去。** 源文件夹是用户的,Quire 在这一步
    /// 只是个读者;写坏了就是毁了用户的东西。
    ///
    /// 两种源文件都认:
    /// - 带 Quire frontmatter 的,字段照搬,但 **id 和文件名重新发**——
    ///   同一个 id 库里只能有一个,沿用旧 id 等于第二篇盖掉第一篇;
    /// - 没有 frontmatter 的,当普通 Markdown 笔记:一级标题当标题,
    ///   没有标题就取第一行。这不是"坏文件",是最常见的一种。
    ///
    /// 已经在库里的同一篇(按地址判重)会跳过并报出来,不当失败处理——
    /// 用户导入的文件夹里有一堆自己以前存过的东西,那是正常情况。
    pub fn import_markdown(&self, dir: &Path) -> Result<ImportReport, VaultError> {
        // 剪藏库自己不能往自己里导。看着像功能,其实是个陷阱:用户点了"导入
        // 剪藏目录"以为能去重,实际得到一堆换了新 id 的副本,原来的还在原地。
        // 更糟的是对着同一个目录反复导,库会越滚越大
        if dir == self.clips_dir() || dir.starts_with(self.clips_dir()) {
            return Err(VaultError::ImportSkippedSelf);
        }
        let mut report = BatchReport::default();
        let mut imported = Vec::new();
        let (paths, unreadable_dirs) = collect_markdown(dir);
        for path in paths {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("(未命名)")
                .to_string();
            // 报告里用的是**源文件名**——用户看的是自己那个文件夹里的东西,
            // 给他一串带 id 的新文件名等于没说
            match self.import_one(&path) {
                Ok(clip) => {
                    report.succeeded.push(name);
                    imported.push(clip.filename);
                }
                Err(e) => report.failed.push(PurgeFailure {
                    filename: name,
                    reason: e.wire(),
                }),
            }
        }
        Ok(ImportReport {
            report,
            imported,
            unreadable_dirs,
        })
    }

    /// 从别的稍后读工具的 CSV / JSON 导入。
    ///
    /// 解析在 [`crate::import`] 里,落盘走 [`Self::store_imported`]——
    /// **判重、编号、摘要兜底这些规则和文件导入共用一份实现**,不走第二条路。
    /// 三类结果分开报:导进来的、库里已有跳过的、读不出来的。混成一句
    /// "导入完成 200 篇"的话,用户分不清那 200 篇是不是他以为的那 200 篇
    pub fn import_data(
        &self,
        text: &str,
        filename: &str,
    ) -> crate::import::DataImportReport {
        let candidates = match crate::import::detect_and_parse(text, filename) {
            Ok(c) => c,
            Err(e) => {
                return crate::import::DataImportReport {
                    error: Some(e),
                    ..Default::default()
                }
            }
        };
        let mut report = crate::import::DataImportReport::default();
        for c in candidates {
            // 行号**在转成 frontmatter 之前就留下来**。`Frontmatter` 是剪藏库的
            // 内部结构,往里塞一个"源文件第几行"是污染——那字段会跟着写进
            // 用户的 Markdown 文件,而它对剪藏本身毫无意义
            let line = c.line;
            let (fm, body) = crate::import::to_frontmatter(c);
            match self.store_imported(fm, body) {
                Ok(clip) => report.imported.push(clip.filename),
                // 重复不是失败。用户导进的文件夹里有一半是存过的,那是正常情况,
                // 报成失败会让他以为导坏了,然后白导第二遍
                Err(VaultError::ImportSkippedDuplicate(title)) => {
                    report.duplicates.push(title)
                }
                Err(e) => report.failed.push(crate::import::ImportFailure {
                    row: line,
                    reason: e.wire().code,
                }),
            }
        }
        report
    }

    fn import_one(&self, path: &Path) -> Result<ClipSummary, VaultError> {
        let original =
            fs::read_to_string(path).map_err(|e| VaultError::Io(std::io::Error::other(e)))?;
        let parsed = match frontmatter::split(&original) {
            // 有 frontmatter 就照单全收,**哪怕里面没有 id**。从 Obsidian 搬过来
            // 的笔记、别的工具导出的文件都长这样:带着自己的一套字段,就是没有
            // Quire 的 id。为这个拒收,等于把最常见的导入场景堵死——id 下面
            // 统一重发就是了
            Some((block, body)) => (Frontmatter::parse(block), body.trim_start().to_string()),
            None => (Frontmatter::default(), original.trim().to_string()),
        };
        self.store_imported(parsed.0, parsed.1)
    }

    /// 把一份 frontmatter + 正文落成一篇剪藏。
    ///
    /// **从 [`Self::import_one`] 里拆出来,是给 CSV/JSON 导入复用的。**
    /// 判重、编号、摘要兜底、文件名去撞——这几步每一步都有测试兜着,
    /// 导入器再写一遍就是同一规则两份实现,迟早不一致:文件导入的去掉了尾斜杠,
    /// CSV 导入的没去掉,于是同一篇文章从两条路进来被判成两篇
    pub fn store_imported(
        &self,
        mut fm: Frontmatter,
        body: String,
    ) -> Result<ClipSummary, VaultError> {
        self.note_write();
        if body.trim().is_empty() {
            return Err(VaultError::EmptyContent);
        }

        // 地址判重。没地址的笔记不参与判重:两段不相干的话地址都是空的,
        // 拿空串去比对就是"第二段永远导不进来"
        if !fm.url.trim().is_empty() {
            if let Some(existing) = self.find_by_url(&fm.url)? {
                return Err(VaultError::ImportSkippedDuplicate(existing.title));
            }
        }

        let now = Local::now();
        let id = fresh_id();
        let date = ids::date_prefix(now.year(), now.month(), now.day());
        let host = slug::host_of(fm.url.trim());
        // 文件名按新 id 重新生成,不可能和库里撞上;真撞上了(同一毫秒同随机)
        // 也不能覆盖,那是最不能忍的一类错
        let mut filename = slug::filename_for(&date, &id, &host);
        let mut n = 1;
        while self.clips_dir().join(&filename).exists() {
            filename = format!("{date}-{id}-{n}-{}.md", slug::site_slug(&host));
            n += 1;
        }

        fm.id = id;
        fm.clipped_at = if fm.clipped_at.trim().is_empty() {
            now.to_rfc3339_opts(SecondsFormat::Secs, false)
        } else {
            fm.clipped_at
        };
        if fm.title.trim().is_empty() {
            fm.title = heading_or_first_line(&body);
        }
        if fm.site.trim().is_empty() {
            // 源文件没写站点就别空着:列表里空站点那一行看着像程序坏了
            fm.site = if host.is_empty() {
                "导入".to_string()
            } else {
                host
            };
        }
        if fm.excerpt.as_deref().is_none_or(|e| e.trim().is_empty()) {
            fm.excerpt = first_paragraph(&body);
        }

        let target = self.clips_dir().join(&filename);
        write_atomic(&target, fm.to_markdown(&body).as_bytes())?;
        Ok(summary_from(fm, filename))
    }

    /// 彻底删除一篇。**这条路没有撤销**,所以只能作用于回收站里的文件。
    /// 「彻底删除」。**只是搬到隔离区,不是抹掉**——见 [`forget`]
    pub fn purge(&self, filename: &str) -> Result<(), VaultError> {
        self.note_write();
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        let from = self.trash_dir().join(filename);
        // **只从回收站搬。** 已经在隔离区里的再点一次"彻底删除",
        // 那样就直接抹掉了,而用户看到的是同一个按钮
        if !from.is_file() {
            return Err(VaultError::NotFound(filename.to_string()));
        }
        let to = self.deleted_dir().join(filename);
        fs::create_dir_all(self.deleted_dir())?;
        if to.exists() {
            // 隔离区里已经有一个同名文件了(用户从备份里塞回来的)。
            // 覆盖就丢东西,报错才是对的
            return Err(VaultError::AlreadyExists(filename.to_string()));
        }
        fs::rename(&from, &to)?;
        Ok(())
    }

    /// **真删。** 全软件唯一不可撤销的操作
    ///
    /// **只能在隔离区里删。** 回收站里的直接删等于把两级退回一级,
    /// 用户点一下「彻底删除」东西当场消失——那正是这次要修的
    pub fn forget(&self, filename: &str) -> Result<(), VaultError> {
        self.note_write();
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        let path = self.deleted_dir().join(filename);
        if !path.is_file() {
            // 只在隔离区里找。同理:回收站里的不许在这儿抹掉
            return Err(VaultError::NotFound(filename.to_string()));
        }
        let id = self.id_of_in(&path);
        fs::remove_file(&path)?;
        if let Some(id) = id {
            self.remove_assets(&id)?;
        }
        Ok(())
    }

    /// 清空隔离区。**这个才是真删**
    pub fn forget_all(&self) -> Result<BatchReport, VaultError> {
        self.note_write();
        let mut report = BatchReport::default();
        let dir = self.deleted_dir();
        if !dir.exists() {
            return Ok(report);
        }
        let names: Vec<String> = fs::read_dir(&dir)?
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .collect();
        for name in names {
            self.record(&mut report, &name, || self.forget(&name));
        }
        Ok(report)
    }

    /// 清掉隔离区里到期的东西。**返回清了几篇**
    ///
    /// 只看隔离区,回收站一概不碰——用户在回收站里放着的东西,
    /// 哪怕放了半年,也不该被程序悄悄清走
    pub fn sweep_deleted(&self) -> Result<usize, VaultError> {
        let dir = self.deleted_dir();
        if !dir.exists() {
            return Ok(0);
        }
        let cutoff = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(QUARANTINE_DAYS as u64 * 86_400))
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let mut removed = 0usize;
        let entries: Vec<_> = fs::read_dir(&dir)?.flatten().collect();
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if modified >= cutoff {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // 清不掉的不打断后面的。一篇文件被别的程序占着,不该让整轮清扫停下
            let _ = self.forget(name);
            if !path.exists() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// 读回收站里某一篇的正文。**只读回收站**,`clips/` 里的读不到。
    ///
    /// 「彻底删除」是整个软件里唯一没有撤销的操作。看不见内容就按下去,
    /// 那不叫确认,叫闭眼签字。
    pub fn read_trash_clip(&self, filename: &str) -> Result<ClipContent, VaultError> {
        if !slug::is_safe_filename(filename) {
            return Err(VaultError::UnsafeFilename(filename.to_string()));
        }
        // 隔离区里的也得能看。看不见内容就让人点"真删",那不叫确认,叫闭眼签字
        let path = self
            .find_in_trash(filename)
            .ok_or_else(|| VaultError::NotFound(filename.to_string()))?;
        let text = fs::read_to_string(&path)?;
        let (block, body) =
            frontmatter::split(&text).ok_or_else(|| VaultError::NotFound(filename.to_string()))?;
        let fm = Frontmatter::parse(block);
        Ok(ClipContent {
            summary: summary_from(fm, filename.to_string()),
            body: body.trim_start().to_string(),
            stamp: content_stamp(&text),
        })
    }

    /// 清空回收站,返回清掉几篇。
    ///
    /// 挨个 `purge` 而不是直接 `remove_dir_all`:图片目录是按 id 分桶的,
    /// 挨个删才能保证 `assets/` 里不给任何一篇留孤儿图。扫不动的条目
    /// 原样留着并记下来——**"清空"没能清干净,必须说出来。**
    /// 清空回收站。**每一篇都搬进隔离区,不是抹掉**
    pub fn empty_trash(&self) -> Result<BatchReport, VaultError> {
        let listing = self.scan_trash()?;
        let mut report = BatchReport::default();
        // **只搬还在回收站里的。** `scan_trash` 现在两处都列,
        // 拿列表直接 purge 的话,隔离区里的会被再"purge"一次——
        // 而 purge 拒绝从隔离区搬,于是每一篇都会报失败
        for item in listing.items.into_iter().filter(|i| !i.quarantined) {
            self.record(&mut report, &item.filename, || self.purge(&item.filename));
        }
        Ok(report)
    }

    /// 记一条批量结果。**一条失败不打断后面的**——打断的话,一次改 30 篇
    /// 在第 3 篇卡住,用户看到的是"改了 3 篇",而剩下 27 篇到底是没改
    /// 还是改了一半,他完全猜不出来。
    fn record<F>(&self, report: &mut BatchReport, filename: &str, run: F)
    where
        F: FnOnce() -> Result<(), VaultError>,
    {
        match run() {
            Ok(()) => report.succeeded.push(filename.to_string()),
            Err(e) => report.failed.push(PurgeFailure {
                filename: filename.to_string(),
                reason: e.wire(),
            }),
        }
    }

    /// 一次改多篇的已读 / 归档标志。
    ///
    /// 挨个 `set_flags`,不是先在内存里算一遍再统一写盘:每个文件的内容
    /// 都不一样(用户自己加过字段、换行风格也不一样),只有走同一条
    /// `set_flags` 路径才碰得到「不许弄坏用户文件」那两条规矩。
    pub fn set_flags_batch(
        &self,
        filenames: &[String],
        read: Option<bool>,
        archived: Option<bool>,
        starred: Option<bool>,
    ) -> Result<BatchReport, VaultError> {
        self.note_write();
        if read.is_none() && archived.is_none() && starred.is_none() {
            // 两项都不改,用户什么也没要求。报"全部成功"是在骗人:那等于
            // 告诉他"你要的改动已经做好了",而实际上一个字节都没动。
            // 这条现在只有程序能触发(界面上三个按钮总有一个带值),
            // 所以它是硬失败而不是提示——调用方该自己先检查参数
            return Err(VaultError::NothingToChange);
        }
        let mut report = BatchReport::default();
        for filename in filenames {
            self.record(&mut report, filename, || {
                self.set_flags(filename, read, archived, starred).map(|_| ())
            });
        }
        Ok(report)
    }

    /// 一次删多篇。**不真删**,挨个搬进回收站,一篇失败不影响其余。
    pub fn trash_batch(&self, filenames: &[String]) -> Result<BatchReport, VaultError> {
        self.note_write();
        let mut report = BatchReport::default();
        for filename in filenames {
            self.record(&mut report, filename, || self.trash(filename));
        }
        Ok(report)
    }

    /// 一次放回多篇,连放回来的剪藏摘要一起给。
    ///
    /// 摘要不是顺带的:前端要靠它把列表补齐,省得为一个撤销重扫整个剪藏库
    /// (重扫还会顺手把刚弹出来的红条清掉)。一篇失败不影响其余——退 20 篇
    /// 卡在第 3 篇就整批不退了,用户还得自己一篇篇捞。
    pub fn restore_batch(&self, filenames: &[String]) -> Result<RestoredBatch, VaultError> {
        self.note_write();
        let mut report = BatchReport::default();
        let mut clips = Vec::new();
        for filename in filenames {
            match self.restore(filename) {
                Ok(clip) => {
                    report.succeeded.push(filename.clone());
                    clips.push(clip);
                }
                Err(e) => report.failed.push(PurgeFailure {
                    filename: filename.clone(),
                    reason: e.wire(),
                }),
            }
        }
        Ok(RestoredBatch { report, clips })
    }

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
            stamp: content_stamp(&text),
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

/// 导出成文件夹的结果。**图片数要报出来**,不然用户拷走之后才发现少了图,
/// 已经是在另一台机器上了——那时候他没法回头查
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFolder {
    pub markdown: String,
    pub images: usize,
    pub failed_images: usize,
}

/// 整个目录拷过去。**目标已存在时只补不删**——用户挑的导出位置可能已经
/// 有一份上次的导出,把文件删了再拷等于替他做决定
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn count_files(dir: &Path) -> usize {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    rd.filter_map(|e| e.ok())
        .map(|e| {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                count_files(&e.path())
            } else {
                1
            }
        })
        .sum()
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
        starred: fm.starred,
        progress: fm.progress,
        tags: fm.tags,
        note: fm.note,
    }
}

/// 把用户敲的标签洗成能安全存进 frontmatter 的样子。
///
/// 洗三样:**首尾空白**(「 rust 」和「rust」在标签栏上是两个标签)、
/// **重复**(同一个词敲两遍,标签栏上出现两个「待读」)、
/// **会撑坏 YAML 的字符**(引号、反斜杠、换行、控制字符)。
///
/// 空的洗完直接扔掉,不留一个空标签占位置。
/// 校验一个标签名合不合法。**长度也要管**:`clean_tags` 只管清洗字符,
/// 不管这个标签有多长,一个 200 字的标签会一路写进 frontmatter、
/// 渲染到标签栏、再渗进搜索索引
fn validate_tag(tag: &str) -> Result<String, VaultError> {
    let trimmed = tag.trim();
    let cleaned = clean_tags(std::slice::from_ref(&trimmed.to_string()));
    match cleaned.first() {
        // 清洗之后不是原来那个,说明原名里有引号/控制符之类被吃掉的字符
        Some(one) if one == trimmed => {
            if one.chars().count() > MAX_TAG_LEN {
                return Err(VaultError::BadTag(format!("标签太长了,最多 {MAX_TAG_LEN} 个字")));
            }
            Ok(one.clone())
        }
        _ => Err(VaultError::BadTag("标签名不合法".to_string())),
    }
}

fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tags.len());
    for raw in tags {
        let cleaned: String = raw
            .trim()
            .chars()
            .filter(|c| !matches!(c, '"' | '\\' | '\n' | '\r' | '\t') && !c.is_control())
            .collect();
        let cleaned = cleaned.trim().to_string();
        if cleaned.is_empty() || out.contains(&cleaned) {
            continue;
        }
        out.push(cleaned);
    }
    out
}

/// 收集要导入的 `.md`。**只挖一层的子目录**,再深就该让用户自己挑了——
/// 点一次导入把整个网盘同步目录扫一遍,那不是导入,那是接管用户硬盘。
/// 点开头的目录一律跳过:`.git`、`.obsidian` 这些不是笔记。
/// 扫一层子目录,找出所有 `.md`。
///
/// **第二个返回值是读不进去的目录。** 一个目录读不出来,里面所有文件就是
/// 整批人间蒸发,而目录级的失败没法塞进"某篇失败"那个列表——那里面装的是
/// 文件。返回出来交给调用方报给用户,总比弹一句"导入完成 240 篇"、
/// 让他以为全导完了要强
fn collect_markdown(dir: &Path) -> (Vec<PathBuf>, Vec<String>) {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, unreadable: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            unreadable.push(dir.display().to_string());
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if depth > 0 {
                    walk(&path, depth - 1, out, unreadable);
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    let mut unreadable = Vec::new();
    walk(dir, 1, &mut out, &mut unreadable);
    // 按文件名排,报错顺序才稳定,用户第二次导入看到的清单和第一次一样
    out.sort();
    unreadable.sort();
    unreadable.dedup();
    (out, unreadable)
}

/// 导入用的标题:先找一级标题,没有就取第一行非空内容。
/// 去掉行首的 `#` 和空白,剩下的就是用户在笔记里写的那个标题。
fn heading_or_first_line(body: &str) -> String {
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let t = t.trim_start_matches('#').trim();
        if !t.is_empty() {
            return t.chars().take(80).collect();
        }
    }
    String::new()
}

/// 导入用的摘要:第一段不是标题、不是代码、不是引用的文字。
/// 复用不了 `excerpt_of`(那是前端的),后端这边只需要一个够用的版本。
fn first_paragraph(body: &str) -> Option<String> {
    let block = body
        .split(
            "

",
        )
        .map(|b| b.trim())
        .find(|b| !b.is_empty() && !b.starts_with(['#', '>', '|', '-', '*', '`', '[']));
    let text = block?
        .replace("![", " ![")
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(120).collect())
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
    fn 回收站里能翻到删掉的那篇() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "留着", "正文")).unwrap();
        let b = v.save(&input("https://b.com", "扔掉的", "正文")).unwrap();
        v.trash(&b.filename).unwrap();

        let items = v.scan_trash().unwrap().items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, b.filename);
        let summary = items[0].summary.as_ref().expect("元数据应读得出来");
        assert_eq!(summary.title, "扔掉的");
        assert!(
            items[0].size_bytes.is_some_and(|n| n > 0),
            "大小得报出来,用户要靠它判断值不值得留"
        );
        assert!(v
            .scan_trash()
            .unwrap()
            .items
            .iter()
            .all(|i| i.filename != a.filename));
        drop(dir);
    }

    #[test]
    fn 空回收站列出来是空的() {
        let (_d, v) = vault();
        // 一次都没删过时 `.trash/` 根本不存在,不能因此报错
        assert!(v.scan_trash().unwrap().items.is_empty());
    }

    #[test]
    fn 读不出元数据的也在回收站里() {
        // 用户可能正拿记事本手动改这些文件。只显示"解析得动的",他会以为
        // 回收站已经空了,而实际上有东西躺在那儿既看不见也删不掉
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        std::fs::create_dir_all(v.trash_dir()).unwrap();
        std::fs::write(
            v.trash_dir().join("2026-09-28-broken.md"),
            "没有 frontmatter 的一段文字",
        )
        .unwrap();

        let items = v.scan_trash().unwrap().items;
        assert_eq!(items.len(), 1, "解析不出来不等于不存在");
        assert!(items[0].summary.is_none());
        drop(dir);
    }

    #[test]
    fn 彻底删除之后回收站里没有了() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input("https://a.com", "确定不要了", "正文"))
            .unwrap();
        v.trash(&saved.filename).unwrap();

        v.purge(&saved.filename).unwrap();

        // 回收站里确实没有了。**但 `scan_trash` 不是空的**——
        // 它连隔离区一起列。原来这条断言写的是 `items.is_empty()`,
        // 那是"彻底删除 = 真删"的语义,现在改成两级之后它就该红了。
        // 这正是要改的地方,不是测试坏了
        assert!(!v.trash_dir().join(&saved.filename).exists());
        let listing = v.scan_trash().unwrap();
        assert_eq!(listing.items.len(), 1, "隔离区里那一篇还看得见");
        assert!(listing.items[0].quarantined);
    }

    #[test]
    fn 彻底删除只认回收站里的文件() {
        // purge 没有撤销。少这一层的话,一个拼错的文件名会静默返回成功,
        // 而库里那篇还好端端躺在那儿——用户以为删了,其实没删
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "还要留着", "正文")).unwrap();

        assert!(matches!(
            v.purge(&saved.filename).unwrap_err(),
            VaultError::NotFound(_)
        ));
        assert!(
            v.clips_dir().join(&saved.filename).exists(),
            "库里的这篇不能被彻底删除"
        );
    }

    #[test]
    fn 彻底删除会连图片一起清掉() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "带图的", "正文")).unwrap();
        let id = v.id_of(&saved.filename).unwrap();
        let bucket = v.assets_dir().join(&id);
        std::fs::create_dir_all(&bucket).unwrap();
        std::fs::write(bucket.join("0.png"), b"\x89PNG").unwrap();
        v.trash(&saved.filename).unwrap();
        // 图片在 `trash()` 那一步就搬到 `.trash/assets/<id>` 了,不是 purge。
        // 这条原来断在 purge 之后,断言的其实是 trash 那步的行为,只是碰巧
        // 挨着——**测试名说的和它验的不是一回事**,所以一并说清
        let trashed_bucket = v.trash_dir().join("assets").join(&id);
        assert!(trashed_bucket.exists(), "移进回收站时图片就该跟着搬走");

        v.purge(&saved.filename).unwrap();
        // 隔离区里的图片**必须还在**。它跟着 `.md` 一起等着,
        // 用户翻了回来,图片当然也得跟着回来,不然正文裂一堆图
        assert!(trashed_bucket.exists(), "只是搬走,图片不该跟着没了");

        v.forget(&saved.filename).unwrap();
        assert!(!trashed_bucket.exists(), "真删之后图片桶也要没");
        drop(dir);
    }

    #[test]
    fn 彻底删除拒收不安全的文件名() {
        let (_d, v) = vault();
        for bad in ["../../.ssh/id_rsa.md", "sub/dir/x.md", "..\\..\\x.md", ""] {
            assert!(
                matches!(v.purge(bad).unwrap_err(), VaultError::UnsafeFilename(_)),
                "{bad} 应该被白名单拦下"
            );
        }
    }

    #[test]
    fn 回收站里能预览正文() {
        // 「彻底删除」是唯一没有撤销的操作,看不见内容就按下去不叫确认,叫闭眼签字
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input("https://a.com", "待删的", "正文里的一句关键内容"))
            .unwrap();
        v.trash(&saved.filename).unwrap();

        let content = v.read_trash_clip(&saved.filename).unwrap();
        assert_eq!(content.body, "正文里的一句关键内容");
        assert_eq!(content.summary.title, "待删的");
    }

    #[test]
    fn 预览只认回收站里的文件() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "还在库里", "正文")).unwrap();
        assert!(matches!(
            v.read_trash_clip(&saved.filename).unwrap_err(),
            VaultError::NotFound(_)
        ));
    }

    #[test]
    fn 预览拒收不安全的文件名() {
        let (_d, v) = vault();
        assert!(matches!(
            v.read_trash_clip("../../.ssh/id_rsa.md").unwrap_err(),
            VaultError::UnsafeFilename(_)
        ));
    }

    #[test]
    fn 清空回收站会报告清不掉的那些() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        for i in 1..=3 {
            let s = v
                .save(&input(&format!("https://a.com/{i}"), "旧剪藏", "正文"))
                .unwrap();
            v.trash(&s.filename).unwrap();
        }
        assert_eq!(v.scan_trash().unwrap().items.len(), 3);

        let report = v.empty_trash().unwrap();
        assert_eq!(report.succeeded.len(), 3);
        assert!(report.failed.is_empty(), "正常情况不该有失败项");
        // **列表不是空的**——三篇都搬进隔离区了,还看得见。
        // 原来这里断言 `items.is_empty()`,那是"清空 = 抹掉"的旧语义
        let listing = v.scan_trash().unwrap();
        assert_eq!(listing.items.len(), 3);
        assert!(listing.items.iter().all(|i| i.quarantined), "都该在隔离区里");
    }

    #[test]
    fn 清空空回收站不算错() {
        let (_d, v) = vault();
        let report = v.empty_trash().unwrap();
        assert!(report.succeeded.is_empty());
        assert!(report.failed.is_empty());
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

        v.set_flags(&name, Some(true), None, None).unwrap();

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

        v.set_flags("mine.md", Some(true), None, None).unwrap();

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

        v.set_flags("crlf.md", Some(true), None, None).unwrap();

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
            v.set_flags("../../evil.md", Some(true), None, None).is_err(),
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
    #[test]
    fn 批量标已读一篇不落() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let names: Vec<String> = (1..=5)
            .map(|i| {
                v.save(&input(&format!("https://a.com/{i}"), "第{i}篇", "正文"))
                    .unwrap()
                    .filename
            })
            .collect();

        let report = v.set_flags_batch(&names, Some(true), None, None).unwrap();
        assert_eq!(report.succeeded.len(), 5);
        assert!(report.failed.is_empty());
        for c in v.scan().unwrap().clips {
            assert!(c.read, "{} 应该是已读", c.filename);
        }
    }

    #[test]
    fn 批量里有一篇不合法不打断其余() {
        // 一次改 30 篇在第 3 篇卡住,用户看到的是"改了 3 篇",而剩下 27 篇
        // 到底改没改他完全猜不出来——所以一条失败不能打断后面的
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let b = v.save(&input("https://a.com/2", "乙", "正文")).unwrap();

        let names = vec![
            a.filename.clone(),
            "不存在的剪藏.md".to_string(),
            b.filename.clone(),
        ];
        let report = v.set_flags_batch(&names, Some(true), None, None).unwrap();
        assert_eq!(
            report.succeeded,
            vec![a.filename.clone(), b.filename.clone()]
        );
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].filename, "不存在的剪藏.md");
        // 断言**代号**而不是那句中文:代号是界面查文案的钥匙,
        // 送空代号的话界面上只会甩一个 `vault.unsafeFilename` 出来。
        // 这条报的不是"不存在"而是"不合法"——文件名带中文,而
        // `is_safe_filename` 只收 ASCII,Quire 自己生成的文件名不会是中文。
        // 顺序也是对的:名字不合法时先说名字有问题,别让人去找一篇不存在的剪藏
        assert_eq!(report.failed[0].reason.code, "vault.unsafeFilename");
        assert_eq!(
            report.failed[0].reason.args.get("detail").map(String::as_str),
            Some("不存在的剪藏.md"),
            "参数里得带上那个文件名,不然界面不知道该说哪个名字不合法"
        );

        let clips = v.scan().unwrap().clips;
        assert!(clips.iter().all(|c| c.read), "两篇真的都改了");
    }

    #[test]
    fn 批量标志同样走不许弄坏用户文件那两条规矩() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input("https://a.com/1", "标题", "正文\n\n第二段"))
            .unwrap();
        // 模拟用户手写过的文件:自己加的字段 + CRLF 换行
        let path = dir.path().join("clips").join(&saved.filename);
        let crlf = "\r\n".repeat(40);
        let mut raw = std::fs::read_to_string(&path).unwrap().replace('\n', &crlf);
        raw.push_str("my_own_field: 留着");
        raw.push_str(&crlf);
        std::fs::write(&path, raw).unwrap();

        v.set_flags_batch(std::slice::from_ref(&saved.filename), Some(true), None, None)
            .unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("my_own_field: 留着"),
            "用户自己的字段不该被顺手整理掉"
        );
        assert!(after.contains(&crlf), "换行风格得原样还原");
        assert!(after.contains("read: true"));
    }

    /// 什么都不该改,就不该重写文件。
    ///
    /// **这条断言跟着产品语义改过。** 原来这里 `set_flags_batch(.., None, None)`
    /// 返回 `Ok` 且报"全部成功",而同一段代码上方的注释写着"报全部成功是在骗人"。
    /// 现在改成硬失败,所以这里断言的是"被拒",而**"文件没被动过"这条
    /// 原本要守的东西一字未改**——那才是这条测试存在的理由
    #[test]
    fn 两项都不改就什么都不做() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/1", "标题", "正文")).unwrap();
        let before = std::fs::metadata(v.clips_dir().join(&saved.filename))
            .unwrap()
            .len();

        v.set_flags_batch(std::slice::from_ref(&saved.filename), None, None, None)
            .unwrap_err();
        assert_eq!(
            std::fs::metadata(v.clips_dir().join(&saved.filename))
                .unwrap()
                .len(),
            before,
            "什么都不该改,就不该重写文件"
        );
    }

    #[test]
    fn 批量归档就是批量归档() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let names: Vec<String> = (1..=3)
            .map(|i| {
                v.save(&input(&format!("https://a.com/{i}"), "第{i}篇", "正文"))
                    .unwrap()
                    .filename
            })
            .collect();

        v.set_flags_batch(&names, None, Some(true), None).unwrap();
        assert!(v.scan().unwrap().clips.iter().all(|c| c.archived));
    }

    #[test]
    fn 批量删除是搬进回收站不是真删() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let names: Vec<String> = (1..=4)
            .map(|i| {
                v.save(&input(&format!("https://a.com/{i}"), "第{i}篇", "正文"))
                    .unwrap()
                    .filename
            })
            .collect();

        let report = v.trash_batch(&names).unwrap();
        assert_eq!(report.succeeded.len(), 4);
        assert!(v.scan().unwrap().clips.is_empty(), "列表里应该空了");
        assert_eq!(v.scan_trash().unwrap().items.len(), 4, "一篇都不能真丢");
    }

    #[test]
    fn 批量删除里的失败项不拖累其余() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let b = v.save(&input("https://a.com/2", "乙", "正文")).unwrap();

        let report = v
            .trash_batch(&[
                "没有这一篇.md".to_string(),
                a.filename.clone(),
                b.filename.clone(),
            ])
            .unwrap();
        assert_eq!(
            report.succeeded,
            vec![a.filename.clone(), b.filename.clone()],
            "两篇都该搬进回收站"
        );
        assert_eq!(report.failed.len(), 1);
        assert!(v.scan().unwrap().clips.is_empty());
    }
    #[test]
    fn 进度能存能读回来() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "长文", "正文")).unwrap();

        v.set_progress(&saved.filename, 0.37).unwrap();

        let back = v.read_clip(&saved.filename).unwrap().summary;
        assert!((back.progress - 0.37).abs() < 1e-6);
        assert!(
            v.scan().unwrap().clips[0].progress > 0.0,
            "列表里也得看得到"
        );
        drop(dir);
    }

    #[test]
    fn 存进度不碰正文也不碰别的字段() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let nl = "\n\n";
        let saved = v
            .save(&input(
                "https://a.com",
                "标题",
                &format!("第一段{nl}第二段"),
            ))
            .unwrap();
        let path = dir.path().join("clips").join(&saved.filename);
        let before = std::fs::read_to_string(&path).unwrap();
        let body_of = |t: &str| t.split_once(nl).map(|(_, b)| b.to_string()).unwrap();

        v.set_progress(&saved.filename, 0.5).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body_of(&before), body_of(&after), "正文一个字节都不能动");
        assert!(after.contains("title: \"标题\""));
        assert!(after.contains("progress: 0.50"));
        drop(dir);
    }

    #[test]
    fn 进度钳在零到一之间() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();

        v.set_progress(&saved.filename, 5.0).unwrap();
        assert_eq!(v.read_clip(&saved.filename).unwrap().summary.progress, 1.0);
        v.set_progress(&saved.filename, -3.0).unwrap();
        assert_eq!(v.read_clip(&saved.filename).unwrap().summary.progress, 0.0);
    }

    #[test]
    fn 没动过就不该动文件() {
        // 滚动一次写一次的话,用户什么都不干也会把 mtime 刷成一片
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();
        let path = dir.path().join("clips").join(&saved.filename);
        let before = std::fs::read_to_string(&path).unwrap();

        v.set_progress(&saved.filename, 0.0).unwrap();
        v.set_progress(&saved.filename, 0.0).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "零进度不该触发写盘"
        );
        drop(dir);
    }

    #[test]
    fn 进度差不到一个百分点就不重写() {
        // 内容上看不出差别——`{:.2}` 格式化之后 0.500 和 0.501 落盘都是 0.50。
        // 但 `write_atomic` 是"写临时文件再改名",文件会被整个换掉:mtime 变了,
        // **文件监控也会被吵醒**,于是"用户只是在滚动"变成一串列表刷新。
        // 所以只能看 mtime。
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();
        let path = dir.path().join("clips").join(&saved.filename);
        v.set_progress(&saved.filename, 0.500).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(1100));
        v.set_progress(&saved.filename, 0.501).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before,
            "肉眼看不出的差别不该重写文件"
        );

        v.set_progress(&saved.filename, 0.60).unwrap();
        assert_ne!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before,
            "真的动了就该写下去"
        );
        drop(dir);
    }

    #[test]
    fn 进度存不了要报错不能装作存上了() {
        // 静默返回"存好了"是最不能忍的一类错:界面会说进度记下了,下次打开
        // 还是从头开始,用户根本不会想到是软件没存
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        assert!(matches!(
            v.set_progress("2026-09-29-missing-example-com.md", 0.5)
                .unwrap_err(),
            VaultError::NotFound(_)
        ));
        assert!(matches!(
            v.set_progress("../../.ssh/id_rsa.md", 0.5).unwrap_err(),
            VaultError::UnsafeFilename(_)
        ));
    }
    /* ── 导入文件夹里的 .md ── */

    /// 一个目录读不出来,里面所有文件就是整批消失。原来只弹"导入完成 N 篇",
    /// 用户以为全导完了,几天后才发现少了东西——报告里连"没找到文件"这个
    /// 概念都没有,他唯一能查的线索是什么都没有
    #[test]
    fn 导入时读不出的目录要报出来() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let missing = std::path::Path::new("Z:/压根不存在的目录/quire");

        let report = v.import_markdown(missing).unwrap();

        assert!(report.report.succeeded.is_empty(), "压根没读到,不该报成功");
        assert_eq!(report.unreadable_dirs.len(), 1, "读不出来的目录得报出来");
    }

    /// 目录都在的时候就没什么好报的,别动不动就往报告里塞东西
    #[test]
    fn 目录都读得到时不报读不出() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let src = dir.path().join("源");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("一.md"), "# 甲\n\n正文").unwrap();

        let report = v.import_markdown(&src).unwrap();

        assert_eq!(report.report.succeeded.len(), 1);
        assert!(report.unreadable_dirs.is_empty(), "没读不出来的就别报");
        drop(dir);
    }

    #[test]
    fn 导入认得Quire格式的文件() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let src = TempDir::new().unwrap();
        let file = src.path().join("旧剪藏.md");
        std::fs::write(
            &file,
            "---
id: \"old1\"
title: \"之前存的那篇\"
url: https://old.example.com/p
             site: old.example.com
excerpt: \"一段摘要\"
author: \"老王\"
             clipped_at: 2025-06-01T10:00:00+08:00
---

正文还在。
",
        )
        .unwrap();

        let report = v.import_markdown(src.path()).unwrap().report;
        assert_eq!(report.succeeded.len(), 1, "报告: {:?}", report);
        let imported = v.scan().unwrap().clips;
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].title, "之前存的那篇");
        assert_eq!(imported[0].url, "https://old.example.com/p");
        assert_eq!(imported[0].excerpt.as_deref(), Some("一段摘要"));
        drop(dir);
    }

    #[test]
    fn 导入给旧文件换个新的id和文件名() {
        // 同一个 id 在库里只能有一个。两份都叫 `2025-06-01-old1-x.md` 的话,
        // 第二篇会直接盖掉第一篇——那是**静默丢数据**
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let src = TempDir::new().unwrap();
        // 两篇带着**同一个旧 id**,但地址不同——测的是 id 撞车那条路
        std::fs::write(
            src.path().join("甲.md"),
            "---
id: \"same\"
title: \"甲\"
url: https://a.com/1
---

正文甲
",
        )
        .unwrap();
        std::fs::write(
            src.path().join("乙.md"),
            "---
id: \"same\"
title: \"乙\"
url: https://b.com/2
---

正文乙
",
        )
        .unwrap();

        let report = v.import_markdown(src.path()).unwrap().report;
        assert!(
            report.failed.is_empty(),
            "两篇地址不同,不该有失败: {:?}",
            report
        );

        let clips = v.scan().unwrap().clips;
        assert_eq!(clips.len(), 2, "两篇都得在");
        assert_ne!(clips[0].filename, clips[1].filename);
        let ids: std::collections::HashSet<&String> = clips.iter().map(|c| &c.id).collect();
        assert_eq!(ids.len(), 2, "id 也得重新发");
        drop(dir);
    }

    #[test]
    fn 没有frontmatter的当纯文本导入() {
        // 用户从别处搬过来的笔记多半没有 frontmatter。那种文件不是"坏文件",
        // 是**正常的一种**——直接拒掉等于把最常见的导入场景堵死
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let src = TempDir::new().unwrap();
        std::fs::write(
            src.path().join("读书笔记.md"),
            "# 深入理解所有权

所有权是 Rust 的核心。
",
        )
        .unwrap();

        v.import_markdown(src.path()).unwrap();

        let clip = v.scan().unwrap().clips.into_iter().next().unwrap();
        assert_eq!(clip.title, "深入理解所有权", "拿一级标题当标题");
        assert_eq!(clip.url, "", "没有地址就老实空着,别编一个");
        assert_eq!(clip.site, "导入");
        let content = v.read_clip(&clip.filename).unwrap();
        assert!(content.body.contains("所有权是 Rust 的核心"));
        drop(dir);
    }

    #[test]
    fn 纯文本实在抽不出标题就用第一行() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let src = TempDir::new().unwrap();
        std::fs::write(
            src.path().join("随手.md"),
            "

今天想到一个点子

展开说说。
",
        )
        .unwrap();

        v.import_markdown(src.path()).unwrap();

        let clip = v.scan().unwrap().clips.into_iter().next().unwrap();
        assert_eq!(clip.title, "今天想到一个点子");
        drop(dir);
    }

    #[test]
    fn 库里已经有的同一篇不重复导入() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com/post", "已经在库里", "正文"))
            .unwrap();
        let src = TempDir::new().unwrap();
        std::fs::write(
            src.path().join("又来了.md"),
            "---
title: \"又是这篇\"
url: https://a.com/post
---

正文
",
        )
        .unwrap();

        let report = v.import_markdown(src.path()).unwrap().report;
        assert!(report.succeeded.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].reason.code, "vault.importSkippedDuplicate");
        assert_eq!(
            report.failed[0].reason.args.get("detail").map(String::as_str),
            // 上面存进去的那篇就叫「已经在库里」,这里得对得上它
            Some("已经在库里"),
            "参数里得带上库里那篇的标题,不然用户不知道跳过去看的是哪篇"
        );
        assert_eq!(v.scan().unwrap().clips.len(), 1);
        drop(dir);
    }

    #[test]
    fn 导入的源文件一个字都不能动() {
        // 源文件夹是用户的,Quire 只是读者。写坏了就是**毁了用户的东西**
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let src = TempDir::new().unwrap();
        let raw = "---
id: \"x\"
title: \"原文\"
url: https://a.com/1
---

原始正文
";
        let file = src.path().join("原文.md");
        std::fs::write(&file, raw).unwrap();

        v.import_markdown(src.path()).unwrap();

        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            raw,
            "源文件必须原封不动"
        );
        drop(dir);
    }

    #[test]
    fn 空的和读不出来的会报出来不假装成功() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let src = TempDir::new().unwrap();
        std::fs::write(
            src.path().join("空的.md"),
            "   

  
",
        )
        .unwrap();
        std::fs::write(src.path().join("非UTF8.md"), [0xff, 0xfe, 0x00, 0x01]).unwrap();

        let report = v.import_markdown(src.path()).unwrap().report;
        assert!(report.succeeded.is_empty());
        assert_eq!(report.failed.len(), 2, "两个都得报出来: {:?}", report);
        drop(dir);
    }

    #[test]
    fn 非md文件不当剪藏() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let src = TempDir::new().unwrap();
        std::fs::write(src.path().join("笔记.txt"), "这是一段笔记").unwrap();
        std::fs::write(
            src.path().join("该读.md"),
            "该读的一篇
",
        )
        .unwrap();

        let report = v.import_markdown(src.path()).unwrap().report;
        assert_eq!(report.succeeded.len(), 1);
        assert!(report.failed.is_empty(), "不是 .md 的直接跳过,不算失败");
        drop(dir);
    }

    #[test]
    fn 子目录里的也导进来() {
        // 用户的笔记不会都摊在一个文件夹里。挖一层的 dot 目录就够了——
        // 再深就该让用户自己挑子目录了
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let src = TempDir::new().unwrap();
        std::fs::create_dir_all(src.path().join("子目录")).unwrap();
        std::fs::write(
            src.path().join("子目录").join("里面的.md"),
            "里面的笔记
",
        )
        .unwrap();

        let report = v.import_markdown(src.path()).unwrap().report;
        assert_eq!(report.succeeded.len(), 1, "报告: {:?}", report);
        drop(dir);
    }

    #[test]
    fn 导不进剪藏库自己的目录() {
        // 把剪藏库导进自己看着像功能,其实是个陷阱:用户以为能去重,
        // 实际得到一堆换了新 id 的副本,原来的还在原地。反复导几次,
        // 库就滚成两倍三倍了
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com/1", "标题", "正文")).unwrap();
        assert_eq!(v.scan().unwrap().clips.len(), 1, "前提:库里确实有东西");

        let err = v.import_markdown(&v.clips_dir()).unwrap_err();
        assert_eq!(err.wire().code, "vault.importSkippedSelf");
        assert_eq!(v.scan().unwrap().clips.len(), 1, "一篇都不能多出来");
        drop(dir);
    }

    // ── 批注 ──

    /// 批注是用户一个字一个字敲出来的。改它的时候正文一个字节都不能动,
    /// 跟改标题、改标签是同一条规矩,不是同一句承诺
    #[test]
    fn 改批注只动frontmatter不碰正文() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文一\n\n正文二")).unwrap();
        let path = dir.path().join("clips").join(&saved.filename);
        let before = std::fs::read_to_string(&path).unwrap();
        let body_before = before.split_once("\n\n").unwrap().1.to_string();

        v.set_note(&saved.filename,  "为什么存它\n第二段", None).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after.split_once("\n\n").unwrap().1, body_before, "正文得原封不动");
        assert_eq!(v.read_clip(&saved.filename).unwrap().summary.note, "为什么存它\n第二段");
        drop(dir);
    }

    #[test]
    fn 批注会被搜到() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "某个标题", "正文")).unwrap();
        v.set_note(&saved.filename,  "这里提到了量子退火", None).unwrap();

        let hits = crate::search::search(&v, "量子退火", 200, None).unwrap().hits;
        assert_eq!(hits.len(), 1, "只写在批注里的词也得能搜到");
        assert_eq!(hits[0].summary.filename, saved.filename);
        drop(dir);
    }

    /// 只写批注不加别的词,搜不到就等于没有。批注是用户写给自己的,
    /// 回头找东西的时候全靠它。
    ///
    /// 前提必须先钉住:那个词在写批注**之前**是搜不到的,否则这条
    /// 测试可能是因为标题或者正文里碰巧有它才绿的——那就等于没测批注
    #[test]
    fn 搜批注的前提是那词只在批注里() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "某个标题", "正文")).unwrap();
        assert!(
            crate::search::search(&v, "量子退火", 200, None).unwrap().hits.is_empty(),
            "前提:写批注之前,这词哪儿都没有"
        );

        v.set_note(&saved.filename,  "这里提到了量子退火", None).unwrap();

        let hits = crate::search::search(&v, "量子退火", 200, None).unwrap().hits;
        assert_eq!(hits.len(), 1, "写进批注之后就该搜得到");
        assert_eq!(hits[0].summary.filename, saved.filename);
        drop(dir);
    }

    #[test]
    fn 清空批注会把值抹掉() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();
        v.set_note(&saved.filename,  "先写点东西", None).unwrap();

        v.set_note(&saved.filename,  "   ", None).unwrap();

        assert_eq!(v.read_clip(&saved.filename).unwrap().summary.note, "");
        assert!(crate::search::search(&v, "先写点东西", 200, None).unwrap().hits.is_empty(), "抹掉了就搜不到");
        drop(dir);
    }

    /// **补全文不能动他在这篇上花过时间的东西。**
    ///
    /// 标签、批注、已读、阅读进度——这些是他剪完之后又回来过好几趟的证据。
    /// 一个"重试抓全文"把它们清掉,那不是帮忙,是**用一次点击抹掉他一段时间**
    #[test]
    fn 补全文不动标签批注和进度() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/1", "标题", "就一小段")).unwrap();
        v.set_tags(&saved.filename, &["重要".to_string()]).unwrap();
        v.set_note(&saved.filename,  "为什么存下这篇", None).unwrap();
        v.set_flags(&saved.filename, Some(false), Some(false), Some(true)).unwrap();
        v.set_progress(&saved.filename, 0.42).unwrap();

        let after = v.set_body(&saved.filename,  "补进来的整篇正文,内容明显更长一些。", None).unwrap();
        assert_eq!(after.tags, vec!["重要".to_string()], "标签没了");
        assert_eq!(after.note, "为什么存下这篇", "批注没了");
        assert!(after.starred, "收藏没了");
        assert!(!after.read, "已读状态变了");
        let back = v.read_clip(&saved.filename).unwrap();
        assert!(
            back.body.contains("整篇正文"),
            "正文没换掉:{}",
            back.body
        );
        drop(dir);
    }

    /// **抓回来的比原来的还短,一个字都不许动。**
    ///
    /// 页面改了、抽出来只剩个壳——那拿它覆盖掉用户已经有的内容是净损失
    #[test]
    fn 更短的正文不覆盖() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/1", "标题", "原来就挺长的一段正文在这里")).unwrap();

        v.set_body(&saved.filename,  "短", None).unwrap();

        let back = v.read_clip(&saved.filename).unwrap();
        assert!(
            back.body.contains("原来就挺长"),
            "短的正文把用户原来的覆盖掉了:{}",
            back.body
        );
        drop(dir);
    }

    /// **空正文要拒。** 存一个空壳进库里,列表上那一条点进去是一片空白,
    /// 而用户以为它是有内容的
    #[test]
    fn 空正文不收() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/1", "标题", "原来的正文")).unwrap();

        assert!(v.set_body(&saved.filename,  "   \n  ", None).is_err());
        assert!(v.read_clip(&saved.filename).unwrap().body.contains("原来的正文"));
        drop(dir);
    }

    /// **只砍首尾空白,中间的换行一个字不动。** 正文里用户自己排的版
    /// (空行、缩进、列表)被重排一遍,他打开就知道出问题了
    #[test]
    fn 补全文不动中间的排版() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/1", "标题", "短")).unwrap();

        v.set_body(&saved.filename,  "\n\n第一段。\n\n- 一\n- 二\n\n  缩进还在\n\n", None).unwrap();

        let body = v.read_clip(&saved.filename).unwrap().body;
        assert!(body.starts_with("第一段。"), "首尾空白没被砍掉:{body:?}");
        assert!(body.contains("- 一\n- 二"), "列表的排版被动了:{body:?}");
        assert!(body.contains("  缩进还在"), "缩进被吃了:{body:?}");
        drop(dir);
    }

    /// **只认图片,不认链接。** 正文里别人手写的超链接不是「Quire 没存好」,
    /// 把它算进去,这一栏就从「我欠了什么」变成了「正文里有什么」,
    /// 而用户点进来是要看第一样的
    #[test]
    fn 远程图只数图不数链接() {
        assert_eq!(remote_image_count("正文里没有图"), 0);
        assert_eq!(remote_image_count("看这里 [原文](https://a.com/x)"), 0);
        assert_eq!(remote_image_count("![封面](https://a.com/1.png)"), 1, "图片没数上");
        assert_eq!(
            remote_image_count("![a](http://x/1.png) ![b](https://x/2.jpg)"),
            2
        );
        // 已经本地化的不该算,那种长这样
        assert_eq!(remote_image_count("![本地](assets/abc/0.png)"), 0);
    }

    /// **一个什么都没有的库,状态是干净的。** 界面只在真有欠账时才摆那个入口,
    /// 所以「空库算不算有欠账」直接决定用户会不会看到一个点进去啥都没有的面板
    #[test]
    fn 空库没有欠账() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let s = v.library_status().unwrap();
        assert_eq!(s.total_clips, 0);
        assert!(s.remote_images.is_empty());
        assert!(s.missing_fulltext.is_empty());
        assert!(s.unreadable.is_empty());
        drop(dir);
    }

    /// **图没下下来的会被数出来。** 这是这一整节存在的理由:
    /// 「数据在你手上」这句话,在带图剪藏导出成一片裂图的时候就是空的,
    /// 而用户唯一能知道自己欠了多少的地方就是这儿
    #[test]
    fn 指着外站的图被数出来() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input(
                "https://a.com/1",
                "带图的文章",
                "![一](https://a.com/1.png)\n\n![二](https://a.com/2.png)",
            ))
            .unwrap();

        let s = v.library_status().unwrap();
        assert_eq!(s.remote_images.len(), 1, "没数出来");
        assert_eq!(s.remote_images[0].count, 2);
        assert_eq!(s.remote_images[0].filename, saved.filename);
        drop(dir);
    }

    /// **图都在本地的不算欠账。** 这一栏要是把本地图也算进去,
    /// 用户点进来看到满屏「没存好」而其实什么都好——那比不摆这个面板更糟
    #[test]
    fn 本地化的图不算欠账() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        v.save(&input(
            "https://a.com/1",
            "图已经下好了",
            "![本地](assets/abc/0.png)",
        ))
        .unwrap();
        assert!(v.library_status().unwrap().remote_images.is_empty());
        drop(dir);
    }

    /// **读不出的文件要报出来,而且理由要说人话。** 用户看到这一条就知道
    /// 「那个文件我得自己去修」,而不是「Quire 少收了一篇」
    #[test]
    fn 读不出的文件被报出来() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        std::fs::write(v.clips_dir().join("坏的.md"), "没有 frontmatter\n").unwrap();

        let s = v.library_status().unwrap();
        assert_eq!(s.unreadable.len(), 1, "读不出的文件没报出来");
        assert!(
            s.unreadable[0].reason.contains("frontmatter"),
            "理由要说人话:{}",
            s.unreadable[0].reason
        );
        drop(dir);
    }

    /// **正文短得不像全文、又有个能抓的地址,才算欠账。**
    ///
    /// 两条都要:只有地址、正文不短的是正常全文;只有短正文、没有地址的是
    /// 用户随手记的一段话(压根没东西可抓),那种报出来就是噪音
    #[test]
    fn 没抓到全文的会被认出来() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let short = v
            .save(&input("https://a.com/1", "只存了个片段", "就一小段话"))
            .unwrap();
        // **没地址的那种不算欠账**:压根没东西可抓,报出来只是噪音
        v.save(&input("", "随手记的", "没有地址的一段话,随手记下来的东西"))
            .unwrap();

        let s = v.library_status().unwrap();
        assert_eq!(s.missing_fulltext.len(), 1, "数出来的不是 1 篇");
        assert_eq!(s.missing_fulltext[0].filename, short.filename);
        drop(dir);
    }

    /// **没有 id 的手工笔记不算欠账,但要报出来。**
    ///
    /// 两件事分开看:它不该被算成「当初没抓到」(**没 id 就没有 Quire 剪藏,
    /// 谈不上抓没抓到**),可它确实是个 Quire 读不了的文件——
    /// 不报出来的话,用户会以为库里的东西 Quire 全都管得着
    #[test]
    fn 手工笔记不算欠账但要报出来() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        std::fs::write(
            v.clips_dir().join("2026-01-01-手写的.md"),
            "---\ntitle: 手写的\nurl: https://a.com/9\n---\n\n就一小段\n",
        )
        .unwrap();
        let s = v.library_status().unwrap();
        assert!(
            s.missing_fulltext.is_empty(),
            "没有 id 的笔记被算成没抓到全文了"
        );
        assert_eq!(
            s.unreadable.len(),
            1,
            "读不了的文件没报出来:用户会以为 Quire 什么都管得着"
        );
        assert!(
            s.unreadable[0].reason.contains("id"),
            "理由要说到点上:{}",
            s.unreadable[0].reason
        );
        drop(dir);
    }

    /// **冲突了就一个字节都不写。** 这条是整个并发防护存在的理由。
    ///
    /// 场景很具体:你打开详情页写了半句批注(还没失焦、还没存),这中间在
    /// Obsidian 里改了正文,回来接着写、失焦保存。不查的话,**丢的不只是
    /// 批注——正文也会一起被写回成旧的那份**,而用户不会发现
    #[test]
    fn 文件在别处改过就什么都不写() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/1", "标题", "原来的正文在这里")).unwrap();

        // 先读一次,拿到指纹
        let read = v.read_clip(&saved.filename).unwrap();
        let stamp = read.stamp.clone();
        assert!(!stamp.is_empty(), "读的时候没给指纹,冲突检查无从谈起");

        // 用户在别处改了正文
        let path = v.clips_dir().join(&saved.filename);
        let raw = std::fs::read_to_string(&path).unwrap();
        let raw = raw.replace("原来的正文在这里", "别人改过的正文");
        std::fs::write(&path, raw).unwrap();

        let err = v
            .set_note(&saved.filename, "我刚写的批注", Some(&stamp))
            .unwrap_err();
        assert!(
            matches!(err, VaultError::ChangedElsewhere),
            "冲突了还写下去,用户刚敲的批注和别人的正文一起丢:{:?}",
            err
        );

        // **磁盘上得原封不动。** 只断言返回 Err 不够——万一它先写后报错呢
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("别人改过的正文"),
            "别人的正文被覆盖了"
        );
        assert!(
            !after.contains("我刚写的批注"),
            "冲突了还把批注写进去了"
        );
        drop(dir);
    }

    /// **标题和补全文走的是同一条闸。** 只给批注加上那道检查的话,
    /// 用户改标题的那一下照样会覆盖掉 Obsidian 里的正文
    #[test]
    fn 标题和补全文也查冲突() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/1", "标题", "一小段")).unwrap();
        let stamp = v.read_clip(&saved.filename).unwrap().stamp;

        let path = v.clips_dir().join(&saved.filename);
        let raw = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            raw.replace("一小段", "别人改过的正文,长得多很多很多"),
        )
        .unwrap();

        assert!(matches!(
            v.set_title(&saved.filename, "我起的新标题", Some(&stamp)),
            Err(VaultError::ChangedElsewhere)
        ));
        assert!(matches!(
            v.set_body(&saved.filename, "我补的正文,也很长很长很长", Some(&stamp)),
            Err(VaultError::ChangedElsewhere)
        ));
        assert!(
            std::fs::read_to_string(&path).unwrap().contains("别人改过的正文"),
            "冲突了还写了"
        );
        drop(dir);
    }

    /// **指纹一样就该放行。** 上一次那条要是把"永远拒绝"写进去,测试照样绿——
    /// 而用户的批注从此再也存不进去
    #[test]
    fn 没被改过就照常写() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/1", "标题", "原来的正文")).unwrap();
        let stamp = v.read_clip(&saved.filename).unwrap().stamp;

        v.set_note(&saved.filename, "正常写一句", Some(&stamp)).unwrap();

        assert_eq!(v.read_clip(&saved.filename).unwrap().summary.note, "正常写一句");
        drop(dir);
    }

    /// **不给指纹就照旧写。** 批量那些路径(打标签、标已读、导入)没有"我在编辑
    /// 这一篇"这个前提,它们拿到的是当场的磁盘内容,不该被一道查不到依据的
    /// 闸拦住
    #[test]
    fn 不给指纹就不拦() {
        let dir = TempDir::new().unwrap();
        let v = Vault::new(dir.path());
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/1", "标题", "正文")).unwrap();

        v.set_note(&saved.filename, "没有指纹也写", None).unwrap();

        assert_eq!(v.read_clip(&saved.filename).unwrap().summary.note, "没有指纹也写");
        drop(dir);
    }

    /// **改一个字节指纹就得变。** 不变的话上面那几条全是死的——
    /// 而"改一个字节"是用户实际会做的事
    #[test]
    fn 指纹跟着内容变() {
        let a = content_stamp("hello");
        assert_eq!(a, content_stamp("hello"), "同样的内容指纹得一样");
        assert_ne!(a, content_stamp("hellp"), "改一个字节指纹就得变");
        assert_ne!(a, content_stamp("hello "), "多一个空格也是变");
        assert_eq!(a.len(), 16, "十六进制 64 位该是 16 个字符");
    }

    /// **进度、已读这类记账变了,不算冲突。**
    ///
    /// 这条是整个守卫能不能用的前提:阅读进度滚动一下写一次,而那正是
    /// Quire 自己写的。要是把它算进指纹,用户开着详情页边读边写批注,
    /// 中间滚了两下,下一次自动存批注就撞上"被别处改过"——报的还是
    /// 一句彻头彻尾的假话。喊狼来了喊三次,用户就不看了
    #[test]
    fn 进度和标记变了不算冲突() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/1", "标题", "正文")).unwrap();

        v.set_progress(&saved.filename, 0.42).unwrap();
        v.set_flags(&saved.filename, Some(true), None, None).unwrap();
        let stamp = {
            let text = std::fs::read_to_string(v.clips_dir().join(&saved.filename)).unwrap();
            content_stamp(&text)
        };

        // 拿最新的指纹去写,不该被拦
        v.set_note(&saved.filename, "读了一半,先记一句", Some(&stamp)).unwrap();
        drop(_d);
    }

    /// 反过来,用户手敲的那几项(标题、批注、标签、正文)改了必须拦住。
    /// 守卫要是只会拦、不会放行,那它拦的就不是冲突,是正常工作
    #[test]
    fn 标题和正文被改过就是冲突() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com/2", "标题", "正文")).unwrap();
        let path = v.clips_dir().join(&saved.filename);
        let stamp = content_stamp(&std::fs::read_to_string(&path).unwrap());

        for (field, from, to) in [
            ("title", "标题", "别人的标题"),
            ("body", "正文", "别人写的正文"),
        ] {
            let text = std::fs::read_to_string(&path).unwrap();
            // **按 frontmatter 里引号包着的那个样子去替换**,别想当然写
            // `title: 标题`——渲染出来是 `title: "标题"`,少一个引号就
            // 一个字都替换不到,而断言"内容没变"反倒先炸了
            let edited = if field == "title" {
                text.replacen(&format!("title: \"{from}\""), &format!("title: \"{to}\""), 1)
            } else {
                text.replacen(from, to, 1)
            };
            assert_ne!(edited, text, "{} 那一下得真改到东西", field);
            std::fs::write(&path, &edited).unwrap();
            let code = v
                .set_note(&saved.filename, "我的批注", Some(&stamp))
                .unwrap_err()
                .wire()
                .code;
            assert_eq!(code, "vault.changedElsewhere", "{field} 改了得拦住");
            // 拦下来之后,批注一个字都不许进磁盘
            let after = std::fs::read_to_string(&path).unwrap();
            assert!(!after.contains("我的批注"), "{field} 冲突时不该写进去");
            std::fs::write(&path, &text).unwrap();
        }
        drop(_d);
    }

    /// 批注是边打字边存的,每敲一下都来一次。没改还去写盘,
    /// 等于白惊动磁盘,还把自己的文件监控吵醒
    #[test]
    fn 批注没变不重写文件() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();
        v.set_note(&saved.filename,  "已经写好的批注", None).unwrap();
        let path = dir.path().join("clips").join(&saved.filename);
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));

        v.set_note(&saved.filename,  "已经写好的批注", None).unwrap();

        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp);
        drop(dir);
    }

    #[test]
    fn 改批注会校验文件名() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        assert_eq!(
            v.set_note("../逃逸.md",  "x", None).unwrap_err().wire().code,
            "vault.unsafeFilename"
        );
    }

    #[test]
    fn 改批注保留用户手写的未知字段() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let path = v.clips_dir().join("mine.md");
        fs::write(
            &path,
            "---\nid: \"mine0001\"\ntitle: \"我的\"\nurl: \"https://a.com/\"\nsite: \"a.com\"\nrating: 5\n---\n\n正文\n",
        )
        .unwrap();

        v.set_note("mine.md",  "我的批注", None).unwrap();

        let rewritten = fs::read_to_string(&path).unwrap();
        let (block, _) = frontmatter::split(&rewritten).unwrap();
        assert!(Frontmatter::parse(block).extra.contains_key("rating"), "rating 不能丢");
    }

    // ── 标签 ──

    #[test]
    fn 改标签只动frontmatter不碰正文() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v
            .save(&input("https://a.com", "标题", "第一段\n\n第二段"))
            .unwrap();
        let path = dir.path().join("clips").join(&saved.filename);
        let before = std::fs::read_to_string(&path).unwrap();

        v.set_tags(&saved.filename, &["rust".into(), "待读".into()])
            .unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        let body_of = |t: &str| {
            t.split_once("\n\n").map(|(_, b)| b.to_string()).unwrap()
        };
        assert_eq!(body_of(&before), body_of(&after), "正文一个字节都不能动");
        assert!(after.contains("tags: [\"rust\", \"待读\"]"), "实际: {after}");
        assert_eq!(v.scan().unwrap().clips[0].tags, vec!["rust", "待读"]);
        drop(dir);
    }

    /// 值没变就不写盘。改标签会在界面上连点好几下,每次都换掉整个文件
    /// 会白白吵醒文件监控,也会让用户的硬盘多一笔无意义的元数据变更。
    #[test]
    fn 标签没变不重写文件() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();
        v.set_tags(&saved.filename, &["rust".into()]).unwrap();
        let path = dir.path().join("clips").join(&saved.filename);
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));

        v.set_tags(&saved.filename, &["rust".into()]).unwrap();

        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(stamp, after, "标签没变却动了文件");
        drop(dir);
    }

    /// 标签是用户敲的,什么都能敲进来。**带引号或换行的标签存下去会让
    ///  整个 frontmatter 解析不出来**,那篇剪藏就此打不开——所以入库前
    ///  必须先洗一遍。
    #[test]
    fn 标签里的引号和换行被洗掉() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();

        let updated = v
            .set_tags(
                &saved.filename,
                &["a\"b".into(), "c\nd".into(), "正常".into()],
            )
            .unwrap();

        assert_eq!(updated.tags, vec!["ab", "cd", "正常"]);
        // 洗过之后必须还能原样读回来
        assert_eq!(v.scan().unwrap().clips[0].tags, vec!["ab", "cd", "正常"]);
        drop(dir);
    }

    #[test]
    fn 标签去重去空白() {
        let cleaned = clean_tags(&[
            "  rust  ".into(),
            "rust".into(),
            "".into(),
            "   ".into(),
            "待读".into(),
        ]);
        assert_eq!(cleaned, vec!["rust", "待读"]);
    }

    /// 标签太多的话,列表上根本排不下,而且多半是在误操作。
    #[test]
    fn 标签太多被拒() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();
        let many: Vec<String> = (0..30).map(|i| format!("t{i}")).collect();
        assert_eq!(
            v.set_tags(&saved.filename, &many).unwrap_err().wire().code,
            "vault.tooManyTags"
        );
        drop(dir);
    }

    #[test]
    fn 改标签会校验文件名() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        assert_eq!(
            v.set_tags("../逃逸.md", &["x".into()]).unwrap_err().wire().code,
            "vault.unsafeFilename"
        );
    }

    /// 标签栏要按篇数倒序排,不然用户建的第一个标签永远排最前,
    /// 后来加的那些(往往是当下真正在用的)要往后面找。
    #[test]
    fn 标签清单按篇数倒序() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let b = v.save(&input("https://a.com/2", "乙", "正文")).unwrap();
        let c = v.save(&input("https://a.com/3", "丙", "正文")).unwrap();
        v.set_tags(&a.filename, &["常用".into(), "少".into()]).unwrap();
        v.set_tags(&b.filename, &["常用".into()]).unwrap();
        v.set_tags(&c.filename, &["常用".into()]).unwrap();

        let tags = v.tag_index().unwrap();
        assert_eq!(tags[0], TagCount { tag: "常用".into(), count: 3 });
        assert_eq!(tags[1], TagCount { tag: "少".into(), count: 1 });
        drop(dir);
    }

    /// 归档和删掉的剪藏不该占着标签栏。标签栏是给「还打算看的那些」用的,
    /// 一堆已归档的老标签混在里面,用户点着点着就以为标签乱了。
    #[test]
    fn 标签栏不含已归档的剪藏() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        v.save(&input("https://a.com/2", "乙", "正文")).unwrap();
        v.set_tags(&a.filename, &["活".into()]).unwrap();
        v.set_flags(&a.filename, None, Some(true), None).unwrap();

        assert!(v.tag_index().unwrap().is_empty(), "归档了的不该占标签栏");
        drop(dir);
    }

    /// 两项都没给,用户什么也没要求。**不能报"全部成功"**——那等于告诉
    /// 他"你要的改动已经做好了",而实际一个字节都没动
    #[test]
    fn 批量改标志两项都没给就拒() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let names = vec![a.filename.clone()];

        assert_eq!(
            v.set_flags_batch(&names, None, None, None).unwrap_err().wire().code,
            "vault.nothingToChange"
        );
        assert!(!v.scan().unwrap().clips[0].read, "不能顺手把它标成已读");
        drop(dir);
    }

    /// 批量打标签。**一条失败不能打断后面的**——一次改 30 篇在第 3 篇
    /// 卡住,用户看到的是"改了 3 篇",剩下 27 篇改没改他完全猜不出来
    #[test]
    fn 批量打标签一条失败不打断其余() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let b = v.save(&input("https://a.com/2", "乙", "正文")).unwrap();
        let c = v.save(&input("https://a.com/3", "丙", "正文")).unwrap();

        let names = vec![
            a.filename.clone(),
            "不存在的剪藏.md".to_string(),
            b.filename.clone(),
            c.filename.clone(),
        ];
        let tags = vec!["待读".to_string(), "rust".to_string()];
        let report = v.set_tags_batch(&names, &tags).unwrap();

        assert_eq!(
            report.succeeded,
            vec![a.filename.clone(), b.filename.clone(), c.filename.clone()]
        );
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].filename, "不存在的剪藏.md");
        for clip in v.scan().unwrap().clips {
            assert_eq!(clip.tags, vec!["待读", "rust"], "{} 的标签", clip.filename);
        }
        drop(dir);
    }

    /// 收藏。**和归档是两码事**:归档是"读完了挪到一边",
    /// 收藏是"一直留着,别混在未读堆里"
    #[test]
    fn 收藏能存能读() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();

        let back = v.set_flags(&a.filename, None, None, Some(true)).unwrap();

        assert!(back.starred, "返回的摘要得已经标上了");
        let reread = v.read_clip(&a.filename).unwrap();
        assert!(reread.summary.starred, "得真的落进文件");
        drop(dir);
    }

    /// 取消收藏
    #[test]
    fn 收藏能取消() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        v.set_flags(&a.filename, None, None, Some(true)).unwrap();

        let back = v.set_flags(&a.filename, None, None, Some(false)).unwrap();

        assert!(!back.starred);
        assert!(!v.read_clip(&a.filename).unwrap().summary.starred);
        drop(dir);
    }

    /// 收藏不影响别的标志。这三个是三件独立的事,用户会只点其中一个
    #[test]
    fn 收藏不动已读和归档() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        v.set_flags(&a.filename, Some(true), Some(true), None).unwrap();

        let back = v.set_flags(&a.filename, None, None, Some(true)).unwrap();

        assert!(back.starred && back.read && back.archived);
        drop(dir);
    }

    /// 值没变就不写盘。批量那条路会连着发几十次,
    /// "把已经标过的再标一遍"太常见了——换一次 mitemtime 会吵醒
    /// 用户的同步盘和备份软件
    #[test]
    fn 标志没变就不重写文件() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        v.set_flags(&a.filename, Some(true), None, Some(true)).unwrap();
        let path = v.clips_dir().join(&a.filename);
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        v.set_flags(&a.filename, Some(true), None, Some(true)).unwrap();

        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "值没变却重写了文件");
        drop(dir);
    }

    /// 批量收藏。一条失败不打断其余,理由和另外两个标志一样
    #[test]
    fn 批量收藏一条失败不打断其余() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let b = v.save(&input("https://a.com/2", "乙", "正文")).unwrap();
        let names = vec![
            a.filename.clone(),
            "不存在的剪藏.md".to_string(),
            b.filename.clone(),
        ];

        let report = v.set_flags_batch(&names, None, None, Some(true)).unwrap();

        assert_eq!(report.succeeded, vec![a.filename.clone(), b.filename.clone()]);
        assert_eq!(report.failed.len(), 1);
        for clip in v.scan().unwrap().clips {
            assert!(clip.starred, "{} 收藏没成功", clip.filename);
        }
        drop(dir);
    }

    /// 三项都不给就拒。收藏加进来之后这条的判据要跟着变——
    /// 只判两项的话,"只收藏但一个值都没传"会被当成成功,而一个字节都没动
    #[test]
    fn 三项都不给就拒() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();

        let code = v
            .set_flags_batch(std::slice::from_ref(&a.filename), None, None, None)
            .unwrap_err()
            .wire()
            .code;
        assert_eq!(code, "vault.nothingToChange");
        drop(dir);
    }

    /// 只给收藏也该被接受——不然界面上"只收藏"这个操作永远报错
    #[test]
    fn 只给收藏不算什么都没做() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();

        let report = v
            .set_flags_batch(std::slice::from_ref(&a.filename), None, None, Some(true))
            .unwrap();

        assert_eq!(report.succeeded.len(), 1);
        assert!(v.read_clip(&a.filename).unwrap().summary.starred);
        drop(dir);
    }

    /// 批量是**替换**,不是追加。用户挑了一堆想统一打上「重要」,
    /// 要是变成追加,他之前那堆五花八门的标签还留着,标签栏会越来越脏
    #[test]
    fn 批量打标签是替换不是追加() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        v.set_tags(&a.filename, &["旧的".into()]).unwrap();

        v.set_tags_batch(std::slice::from_ref(&a.filename), &["新的".into()]).unwrap();

        assert_eq!(v.scan().unwrap().clips[0].tags, vec!["新的"]);
        drop(dir);
    }

    /// 撤销一次批量删除。单篇删了有按钮退回来,一次删 20 篇没有——
    /// 唯一的保险变成"下次少勾两篇",那不叫保险
    #[test]
    fn 批量撤销能把整批放回来() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let b = v.save(&input("https://a.com/2", "乙", "正文")).unwrap();
        let names = vec![a.filename.clone(), b.filename.clone()];

        v.trash_batch(&names).unwrap();
        assert!(v.scan().unwrap().clips.is_empty(), "先确认确实删掉了");

        let out = v.restore_batch(&names).unwrap();
        let report = &out.report;

        assert_eq!(report.failed.len(), 0, "不该有放不回来的");
        assert_eq!(report.succeeded, names);
        // 摘要跟着回来,前端不用为一个撤销重扫整个剪藏库
        assert_eq!(out.clips.len(), 2, "两篇的摘要都得带回来");
        let titles: Vec<&str> = out.clips.iter().map(|c| c.title.as_str()).collect();
        assert!(titles.contains(&"甲") && titles.contains(&"乙"), "摘要里要看得见标题,got {titles:?}");
        let back = v.scan().unwrap();
        assert_eq!(back.clips.len(), 2, "两篇都该回到列表里");
        assert!(v.scan_trash().unwrap().items.is_empty(), "回收站该空了");
        drop(dir);
    }

    /// 撤销整批时中间有一篇放不回来,**后面的照旧要放**。
    /// 一次退 20 篇,第 3 篇卡住就整批不退了,用户还得自己一篇篇捞
    #[test]
    fn 批量撤销一条失败不打断其余() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let b = v.save(&input("https://a.com/2", "乙", "正文")).unwrap();
        let c = v.save(&input("https://a.com/3", "丙", "正文")).unwrap();
        let names = vec![a.filename.clone(), b.filename.clone(), c.filename.clone()];
        v.trash_batch(&names).unwrap();
        // 乙这篇原位先被占了(用户手工放回来过),撤销时它必然撞车
        fs::write(v.clips_dir().join(&b.filename), b"handwritten").unwrap();

        let report = &v
            .restore_batch(&[
                a.filename.clone(),
                b.filename.clone(),
                c.filename.clone(),
            ])
            .unwrap()
            .report;

        assert_eq!(report.succeeded, vec![a.filename.clone(), c.filename.clone()]);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].filename, b.filename);
        assert_eq!(
            report.failed[0].reason.code, "vault.alreadyExists",
            "原位被占了得说清楚,不能报一句'失败了'"
        );
        // 关键:丙还是被放回来了,没被乙的失败带下去
        assert!(v.clips_dir().join(&c.filename).is_file(), "丙必须回到原位");
        // 用户手写那份不能被覆盖
        assert_eq!(
            fs::read_to_string(v.clips_dir().join(&b.filename)).unwrap(),
            "handwritten"
        );
        drop(dir);
    }

    /// 撤销时传进来的名字可能压根不在回收站(用户自己把文件拿走了)。
    /// 这种情况该报出来,不该当成成功——放不回来就是放不回来
    #[test]
    fn 批量撤销放不回来的那篇得报出来() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let names = vec![a.filename.clone()];

        let report = &v.restore_batch(&names).unwrap().report;

        assert!(report.succeeded.is_empty(), "回收站里没有,不该说成功");
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].filename, a.filename);
        assert_eq!(report.failed[0].reason.code, "vault.notFound");
        drop(dir);
    }

    /// 图片跟着一起回来。用户在详情页看得到图,撤销完图没了的话,
    /// 他会以为 Quire 把他的图片删了
    #[test]
    fn 批量撤销把图片也搬回来() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let id = v.id_of(&a.filename).expect("剪藏得有 id");
        let live = v.assets_dir().join(&id);
        fs::create_dir_all(&live).unwrap();
        fs::write(live.join("cover.png"), b"PNG").unwrap();

        v.trash_batch(std::slice::from_ref(&a.filename)).unwrap();
        assert!(!live.join("cover.png").is_file(), "先确认图进了回收站");

        v.restore_batch(std::slice::from_ref(&a.filename)).unwrap();

        assert_eq!(fs::read(live.join("cover.png")).unwrap(), b"PNG");
        drop(dir);
    }

    /// 空名单是"没什么要撤销",不是故障
    #[test]
    fn 批量撤销空名单不算失败() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let out = v.restore_batch(&[]).unwrap();
        assert!(out.report.succeeded.is_empty() && out.report.failed.is_empty() && out.clips.is_empty());
        drop(dir);
    }

    /// 标签压根没改的时候,一批文件一个字节都不该动。批量是最容易
    /// 写出无谓写入的地方:用户在标签栏上点了一下「已确认」,其实没改任何东西
    #[test]
    fn 批量打标签值没变不重写() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        v.set_tags(&a.filename, &["待读".into()]).unwrap();
        let path = dir.path().join("clips").join(&a.filename);
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));

        v.set_tags_batch(std::slice::from_ref(&a.filename), &["待读".into()]).unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            stamp,
            "标签没变却动了文件"
        );
        drop(dir);
    }

    #[test]
    fn 批量打标签标签超了要报错() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com/1", "甲", "正文")).unwrap();
        let many: Vec<String> = (0..30).map(|i| format!("t{i}")).collect();
        assert_eq!(
            v.set_tags_batch(std::slice::from_ref(&a.filename), &many)
                .unwrap_err()
                .wire()
                .code,
            "vault.tooManyTags"
        );
        drop(dir);
    }

    // ── 改标题 ──

    #[test]
    fn 改标题只动frontmatter() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "原来的标题", "正文一\n\n正文二")).unwrap();
        let path = dir.path().join("clips").join(&saved.filename);
        let before = std::fs::read_to_string(&path).unwrap();

        let updated = v.set_title(&saved.filename,  "我自己的标题", None).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        let body_of = |t: &str| {
            t.split_once("\n\n").map(|(_, b)| b.to_string()).unwrap()
        };
        assert_eq!(body_of(&before), body_of(&after), "正文一个字节都不能动");
        assert_eq!(updated.title, "我自己的标题");
        assert_eq!(v.scan().unwrap().clips[0].title, "我自己的标题");
        // 别的字段不能被动:地址、站点、标志都得原封不动
        assert_eq!(updated.url, "https://a.com");
        assert!(!updated.read);
        drop(dir);
    }

    /// 标题是要写进 .md 也要显示在界面上的。敲个引号进去,拼出来的
    /// frontmatter 解析不出来,这篇剪藏从此打不开——在入口挡住,
    /// 别指望用户自己注意
    #[test]
    fn 标题里的引号能原样存回去() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();
        let tricky = r#"他说"这是重点"，不是"那个""#;

        v.set_title(&saved.filename,  tricky, None).unwrap();

        assert_eq!(v.scan().unwrap().clips[0].title, tricky);
        // 还能原样读回来,说明 frontmatter 没被撑坏
        assert_eq!(v.read_clip(&saved.filename).unwrap().summary.title, tricky);
        drop(dir);
    }

    /// 空标题在列表上就是一行空白,用户会以为软件坏了。
    /// 宁可拒绝,也不能存一个空标题下去
    #[test]
    fn 空标题被拒() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();
        assert_eq!(
            v.set_title(&saved.filename,  "   ", None).unwrap_err().wire().code,
            "vault.emptyTitle"
        );
        assert_eq!(v.scan().unwrap().clips[0].title, "标题", "拒了就不能改掉");
        drop(dir);
    }

    /// 改标题和改标志走的是同一段代码,但**同一段代码不等于同一份保障**。
    /// 这里照着 `改已读状态保留用户手写的未知字段` 再锁一遍:少了这条,
    /// 哪天有人把渲染逻辑挪了地方,字段被静默吃掉,测试一个都不会红
    #[test]
    fn 改标题保留用户手写的未知字段() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let path = v.clips_dir().join("mine.md");
        fs::write(
            &path,
            "---\nid: \"mine0001\"\ntitle: \"我的\"\nurl: \"https://a.com/\"\nsite: \"a.com\"\nrating: 5\nstatus: \"在读\"\n---\n\n正文\n",
        )
        .unwrap();

        v.set_title("mine.md",  "我自己起的名字", None).unwrap();

        let rewritten = fs::read_to_string(&path).unwrap();
        let (block, _) = frontmatter::split(&rewritten).unwrap();
        let fm = Frontmatter::parse(block);
        assert!(fm.extra.contains_key("rating"), "rating 是用户自己加的,不能丢");
        assert!(fm.extra.contains_key("status"), "status 同理");
    }

    #[test]
    fn 改标题保留CRLF换行风格() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let path = v.clips_dir().join("crlf.md");
        fs::write(
            &path,
            "---\r\nid: \"crlf0001\"\r\ntitle: \"旧的\"\r\nurl: \"https://a.com/\"\r\nsite: \"a.com\"\r\n---\r\n\r\n正文一行\r\n正文二行\r\n",
        )
        .unwrap();

        v.set_title("crlf.md",  "新的", None).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        // 换行的**条数**会变(render 会把标准字段补齐),要锁的是**风格**:
        // 全文每一个 \n 都得跟在 \r 后面,一个裸 LF 都不能混进来
        let bytes = after.as_bytes();
        let lone = bytes
            .iter()
            .enumerate()
            .filter(|(i, b)| **b == b'\n' && (*i == 0 || bytes[i - 1] != b'\r'))
            .count();
        assert_eq!(lone, 0, "不该出现裸 LF,整篇都得保持 CRLF");
        assert!(after.ends_with("\r\n正文一行\r\n正文二行\r\n"), "正文原样");
        assert!(after.contains("title: \"新的\""), "标题确实改了");
    }

    #[test]
    fn 标题没变不重写文件() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "标题", "正文")).unwrap();
        let path = dir.path().join("clips").join(&saved.filename);
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));

        v.set_title(&saved.filename,  "标题", None).unwrap();

        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp);
        drop(dir);
    }

    #[test]
    fn 改标题会校验文件名() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        assert_eq!(
            v.set_title("../逃逸.md",  "x", None).unwrap_err().wire().code,
            "vault.unsafeFilename"
        );
    }

    // ── 标签重命名/合并 ───────────────────────────────────────────
    //
    // 标签攒到几十个之后必然会烂尾:`rust` / `Rust` / `编程` / `待读`
    // 混在一起,用户唯一的办法是逐篇点开手改。攒到几百篇那天他就放弃整理了,
    // 标签栏从此变成一坨没人点的垃圾。所以合并是**必须有**的,不是锦上添花

    /// 重命名之后,新标签出现、旧标签消失
    #[test]
    fn 标签能重命名() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        v.set_tags(&a.filename, &["rust".into(), "待读".into()]).unwrap();

        let r = v.rename_tag("rust", "Rust").unwrap();

        assert_eq!(r.changed, 1, "该改一篇");
        let tags = v.scan().unwrap().clips[0].tags.clone();
        assert!(tags.contains(&"Rust".to_string()), "新标签没进去: {tags:?}");
        assert!(!tags.contains(&"rust".to_string()), "旧标签还在: {tags:?}");
        assert!(tags.contains(&"待读".to_string()), "别的标签被误伤了: {tags:?}");
        drop(dir);
    }

    /// 合并:新标签已经有了,合并之后是**并集**,不是替换。
    /// 用户把「待读」并进「重要」时,两边的文章都该在「重要」底下
    #[test]
    fn 合并标签是取并集不是覆盖() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        let b = v.save(&input("https://b.com", "乙", "正文")).unwrap();
        let c = v.save(&input("https://c.com", "丙", "正文")).unwrap();
        v.set_tags(&a.filename, &["重要".into()]).unwrap();
        v.set_tags(&b.filename, &["待读".into()]).unwrap();
        // **这一篇才是关键。** 原先的样本里没有一篇同时带两个标签,
        // 于是"取并集"和"直接覆盖"算出来一模一样,那条测试等于没测
        v.set_tags(&c.filename, &["重要".into(), "待读".into(), "其他".into()]).unwrap();

        let r = v.rename_tag("待读", "重要").unwrap();

        assert_eq!(r.changed, 2, "带旧标签的两篇都要改");
        let clips = v.scan().unwrap().clips;
        let find = |name: &str| {
            clips
                .iter()
                .find(|x| x.filename == name)
                .map(|x| x.tags.clone())
                .unwrap()
        };
        assert_eq!(find(&a.filename), vec!["重要".to_string()]);
        assert_eq!(find(&b.filename), vec!["重要".to_string()]);
        // 同一篇里出现两次「重要」,标签栏就会显示成「重要 2」而实际只有 1 篇
        assert_eq!(
            find(&c.filename),
            vec!["重要".to_string(), "其他".to_string()],
            "合并不该在同一篇里留下两个同名标签"
        );
        drop(dir);
    }

    /// **绝不弄坏用户的文件。** 这条和 `set_tags` 是同一套规矩,
    /// 但重命名是**批量**触发的,一旦漏了,一晚上能毁掉半个剪藏库
    #[test]
    fn 重命名标签不碰正文和未知字段() {
        let (_d, v) = vault();
        v.ensure_dirs().unwrap();
        let path = v.clips_dir().join("mine.md");
        fs::write(
            &path,
            "---
id: abc123
title: \"我的笔记\"
my_own_field: keep-me
tags: [rust]
---

正文第一行

  缩进和空行都要原样留着
",
        )
        .unwrap();
        let before = fs::read_to_string(&path).unwrap();

        v.rename_tag("rust", "Rust").unwrap();

        let after = fs::read_to_string(&path).unwrap();
        // **别按字面比 `my_own_field: keep-me`**——渲染时会补引号,
        // 那是正常规范化,不是弄坏了。盯的是字段还在、值还在
        assert!(after.contains("my_own_field"), "用户手写的字段没了");
        assert!(after.contains("keep-me"), "字段的值没了");
        assert!(after.contains("正文第一行"), "正文没了");
        assert!(after.contains("  缩进和空行都要原样留着"), "正文的缩进被动了");
        assert_ne!(before, after, "该改的没改");
        drop(_d);
    }

    /// 标签顺序是用户自己排的,重命名不许偷偷重排
    #[test]
    fn 重命名不动标签顺序() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        v.set_tags(&a.filename, &["重要".into(), "rust".into(), "待读".into()]).unwrap();

        v.rename_tag("rust", "RUST").unwrap();

        assert_eq!(
            v.scan().unwrap().clips[0].tags,
            vec!["重要".to_string(), "RUST".to_string(), "待读".to_string()]
        );
        drop(dir);
    }

    /// **没改就不写。** 重命名是个批量动作,哪怕只有一篇带旧标签,
    /// 剩下的也一个字节都不该动——`write_atomic` 改 mtime,
    /// 而 mtiem 变了会惊动用户的网盘同步,几百个文件同时重传
    #[test]
    fn 没带旧标签的文件不重写() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "带标签的", "正文")).unwrap();
        let b = v.save(&input("https://b.com", "没标签的", "正文")).unwrap();
        v.set_tags(&a.filename, &["rust".into()]).unwrap();
        let path = v.clips_dir().join(&b.filename);
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));

        v.rename_tag("rust", "Rust").unwrap();

        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp,
                   "没带旧标签的那篇被重写了");
        drop(dir);
    }

    /// 回收站里的也算。放回来之后还是旧标签,用户会觉得「合了等于没合」
    #[test]
    fn 回收站里的也跟着改() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        v.set_tags(&a.filename, &["rust".into()]).unwrap();
        // **归档只是打个标记,挪文件的是 `trash()`**。用错 API 的后果是
        // 测试看着像"回收站没跟着改",其实是文件压根没进回收站
        v.trash(&a.filename).unwrap();

        let r = v.rename_tag("rust", "Rust").unwrap();

        assert_eq!(r.changed, 1, "回收站里那篇也要改");
        let text = fs::read_to_string(v.trash_dir().join(&a.filename)).unwrap();
        assert!(text.contains("Rust"), "回收站里的没改: {text}");
        assert!(!text.contains("rust:"), "回收站里的还是旧标签");
        drop(dir);
    }

    /// 旧标签和新标签只差大小写时,得**真的**改。
    /// 直接 `==` 比会判成"没变"于是跳过写入,用户点了没反应
    #[test]
    fn 只差大小写也要改() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        v.set_tags(&a.filename, &["rust".into()]).unwrap();

        let r = v.rename_tag("rust", "Rust").unwrap();

        assert_eq!(r.changed, 1, "大小写不同就该改");
        assert_eq!(v.scan().unwrap().clips[0].tags, vec!["Rust".to_string()]);
        drop(dir);
    }

    /// 新标签不合规矩(太长、太多)要拦住,而且**一篇都不许改**
    #[test]
    fn 新的标签名不合法就一篇都不改() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        v.set_tags(&a.filename, &["rust".into()]).unwrap();
        let path = v.clips_dir().join(&a.filename);
        let before = fs::read_to_string(&path).unwrap();

        assert!(v.rename_tag("rust", &"x".repeat(200)).is_err(), "超长标签该被拒");
        assert_eq!(fs::read_to_string(&path).unwrap(), before, "被拒了却已经改了文件");
        drop(dir);
    }

    /// 超长的标签,改标签那条路也得拦。
    /// **两条入口规则必须一样**:合并走 `rename_tag`、改标签走 `set_tags`,
    /// 一个放行一个不放行,用户只会觉得这软件有毛病
    #[test]
    fn 超长标签两条入口都拦() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        let long = "x".repeat(MAX_TAG_LEN + 1);

        assert_eq!(
            v.set_tags(&a.filename, std::slice::from_ref(&long)).unwrap_err().wire().code,
            "vault.badTag"
        );
        assert_eq!(
            v.rename_tag("rust", &long).unwrap_err().wire().code,
            "vault.badTag"
        );
        drop(dir);
    }

    /// 刚好到上限的要放行。上限是闸不是墙,差一个字就报错是另一种毛病
    #[test]
    fn 刚好到上限的放行() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        let exact = "字".repeat(MAX_TAG_LEN);

        v.set_tags(&a.filename, std::slice::from_ref(&exact)).unwrap();

        assert_eq!(v.scan().unwrap().clips[0].tags, vec![exact]);
        drop(dir);
    }

    /// 读不出的文件得报出来,不能悄悄跳——和搜索、扫描一个规矩
    #[test]
    fn 读不出的文件要报出来() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "好的一篇", "正文")).unwrap();
        v.set_tags(&a.filename, &["rust".into()]).unwrap();
        fs::write(v.clips_dir().join("坏文件.md"), [0xff, 0xfe, 0x00]).unwrap();

        let r = v.rename_tag("rust", "Rust").unwrap();

        assert_eq!(r.changed, 1, "好的一篇该改成功");
        assert_eq!(r.failed.len(), 1, "读不出的那篇得报出来");
        assert_eq!(r.failed[0].filename, "坏文件.md");
        drop(dir);
    }


    // ── 隔离区 ────────────────────────────────────────────────────
    //
    // 产品承诺是「误删了能一键撤销」。原来这条承诺在**伤害最大的地方断了**:
    // 移进回收站有撤销,「彻底删除」和「清空回收站」只有一句确认,点下去就没了。
    // 现在改成两级:回收站 → 隔离区 → 真删,第二级才有「现在真删」的出口

    #[test]
    fn 彻底删除只是移到隔离区() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        v.trash(&a.filename).unwrap();

        v.purge(&a.filename).unwrap();

        assert!(!v.trash_dir().join(&a.filename).exists(), "不该留在回收站里");
        assert!(
            v.deleted_dir().join(&a.filename).exists(),
            "隔离区里应该有它——点一下彻底删除就把东西抹了,承诺就是空话"
        );
        drop(dir);
    }

    /// **隔离区里的还能翻回来。** 这是整个改动的意义所在
    #[test]
    fn 隔离区里的能翻回原位() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        v.trash(&a.filename).unwrap();
        v.purge(&a.filename).unwrap();

        let back = v.restore(&a.filename).unwrap();

        assert_eq!(back.filename, a.filename);
        assert!(v.clips_dir().join(&a.filename).exists(), "没放回剪藏库");
        assert!(!v.deleted_dir().join(&a.filename).exists(), "隔离区里还留着");
        drop(dir);
    }

    /// 「现在真删」必须真的删干净。这是全软件**唯一**不可撤销的操作
    #[test]
    fn 真删是真的删() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        v.trash(&a.filename).unwrap();
        v.purge(&a.filename).unwrap();

        v.forget(&a.filename).unwrap();

        assert!(!v.deleted_dir().join(&a.filename).exists(), "还在");
        assert!(!v.trash_dir().join(&a.filename).exists(), "回收站里也不该有");
        drop(dir);
    }

    /// **只能在隔离区里真删。** 回收站里的直接真删,等于把两级退回一级,
    /// 用户点「彻底删除」的东西当场消失——那正是这次要修的
    #[test]
    fn 回收站里的不许直接真删() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        v.trash(&a.filename).unwrap();

        assert!(
            v.forget(&a.filename).is_err(),
            "回收站里的能直接抹掉,两级就白设了"
        );
        assert!(v.trash_dir().join(&a.filename).exists(), "它被抹掉了");
        drop(dir);
    }

    /// 回收站视图要**把两处都列出来**,并说清哪一篇已经在隔离区
    #[test]
    fn 回收站视图两处都列() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        let b = v.save(&input("https://b.com", "乙", "正文")).unwrap();
        v.trash(&a.filename).unwrap();
        v.trash(&b.filename).unwrap();
        v.purge(&a.filename).unwrap();

        let listing = v.scan_trash().unwrap();

        assert_eq!(listing.items.len(), 2, "两篇都要看得见");
        let in_trash = listing.items.iter().find(|i| i.filename == b.filename).unwrap();
        let in_quarantine = listing.items.iter().find(|i| i.filename == a.filename).unwrap();
        assert!(!in_trash.quarantined, "还在回收站里的不该标成已隔离");
        assert!(in_quarantine.quarantined, "隔离区里的没标出来");
        drop(dir);
    }

    /// 界面上那个"30 天"是从这儿来的。**单独钉一条**,是因为它防的是
    /// 一种特别难发现的坏:改了 `QUARANTINE_DAYS` 常量,后端行为全对,
    /// 只有界面上那句话悄悄变成了假话,而没有任何一个既有测试会红
    #[test]
    fn 保留期下发的是常量本身() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let listing = v.scan_trash().unwrap();
        assert_eq!(
            listing.quarantine_days, QUARANTINE_DAYS,
            "前端显示的保留期必须跟着常量走"
        );
        drop(dir);
    }

    /// **端到端:CSV 导进去,盘上真有文件。** 解析器返回了什么不重要,
    /// 重要的是写出来的那一篇能不能被 `scan` 读回来——那才是用户看到的东西
    #[test]
    fn pocket_csv真的落盘() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let nl = String::from_utf8(vec![10]).expect("10 是合法 UTF-8");
        let csv = [
            "title,url,time_added,tags,status",
            "\"C++ 入门,第二版\",https://a.com,1609459200,阅读|编程,unread",
            "Rust 所有权,https://b.com,1609545600,编程,archive",
        ]
        .join(&nl);

        let report = v.import_data(&csv, "pocket.csv");

        assert_eq!(report.imported.len(), 2, "两篇都该导进来");
        let clips = v.scan().unwrap().clips;
        assert_eq!(clips.len(), 2, "盘上也得有两篇");
        let titled = clips.iter().find(|c| c.title == "C++ 入门,第二版").unwrap();
        assert_eq!(titled.url, "https://a.com");
        assert_eq!(titled.tags, vec!["阅读", "编程"], "标签要跟着过来");
        assert!(!titled.archived);
        assert!(
            clips.iter().any(|c| c.archived),
            "Pocket 里 status=archive 的那篇该是归档态"
        );
        drop(dir);
    }

    /// **没正文也要导得进来**。Pocket 压根不存正文,不为这个造一篇空的,
    /// 那篇在列表里点开是空的,用户会以为导坏了
    #[test]
    fn pocket没正文的也导得进来() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let csv = ["title,url", "只有标题,https://a.com"].join("
");

        let report = v.import_data(&csv, "pocket.csv");

        assert_eq!(report.imported.len(), 1);
        let clip = v.scan().unwrap().clips.remove(0);
        // **正文里得真有东西,而且是拿他自己数据拼的。** 只断言"导进来了"
        // 是弱测试:合成逻辑整个挂掉、body 变成空串,这条一样绿——
        // 而用户点开那一篇看到的是一片空白,他还以为导入把内容弄丢了
        let text = std::fs::read_to_string(
            v.clips_dir().join(&clip.filename),
        )
        .unwrap();
        assert!(
            text.contains("https://a.com"),
            "合成正文里得指着原文的地址,不然用户点开什么也没有:{}",
            text
        );
        assert!(
            text.contains("只有标题"),
            "标题也该在正文里,不然列表和正文对不上:{}",
            text
        );
        drop(dir);
    }

    /// **重跑一次不产生第二篇。**用户导到一半发现漏了文件,重新导一次是常事;
    /// 每重跑一次库里就多一份副本,那这个功能就是负分
    #[test]
    fn 重复导同一份不产生第二篇() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let csv = ["title,url", "甲文,https://a.com"].join("
");

        let first = v.import_data(&csv, "pocket.csv");
        let second = v.import_data(&csv, "pocket.csv");

        assert_eq!(first.imported.len(), 1);
        assert_eq!(second.imported.len(), 0, "第二遍不该再导");
        assert_eq!(second.duplicates.len(), 1, "得报成重复,不是失败");
        assert!(second.failed.is_empty(), "重复不算失败");
        assert_eq!(v.scan().unwrap().clips.len(), 1, "库里还是一篇");
        drop(dir);
    }

    /// 格式不认的时候**整批停下并说清楚**,不是导进零篇然后报"成功"
    #[test]
    fn 不认的格式整批停下() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();

        let report = v.import_data("随便什么", "书签.xhtml");

        assert!(report.imported.is_empty());
        assert!(
            report.error.is_some(),
            "得有个说法,不能静默导零篇"
        );
        drop(dir);
    }

    /// **通用 CSV 也走得通**,不只是 Pocket 那一种列名
    #[test]
    fn 通用csv也走得通() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let csv = ["标题,链接,标签,日期", "甲文,https://a.com,阅读,2023-05-01"].join("
");

        let report = v.import_data(&csv, "导出.csv");

        assert_eq!(report.imported.len(), 1);
        let clips = v.scan().unwrap().clips;
        assert_eq!(clips[0].title, "甲文");
        assert!(clips[0].clipped_at.starts_with("2023-05-01"), "{}", clips[0].clipped_at);
        drop(dir);
    }

    /// **导出的文件夹里图片要跟着走。** 这是本功能的全部理由:
    /// 剪藏时图片存成了 `assets/<id>/3.png` 这种相对路径,只导出单个 .md
    /// 的话,用户把文件发给别人、在手机上打开,一张图都裂——
    /// 而"我的数据是我的"这句话恰恰在带图剪藏上最该成立
    #[test]
    fn 导出的文件夹里图片跟着走() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "甲", "正文甲")).unwrap();
        // 造一张图,放在这篇该在的位置
        let img_dir = v.assets_dir().join(v.id_of(&saved.filename).unwrap());
        std::fs::create_dir_all(&img_dir).unwrap();
        std::fs::write(img_dir.join("0.png"), b"PNGDATA").unwrap();

        let out = TempDir::new().unwrap();
        let report = v.export_folder(out.path(), None).unwrap();

        assert_eq!(report.images, 1, "图没拷过去:{:?}", report);
        let copied = out.path().join("assets");
        assert!(copied.is_dir(), "少了 assets 目录");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&copied)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        dirs.sort();
        assert_eq!(dirs.len(), 1, "每篇一个自己的目录");
        let inner = dirs[0].join("0.png");
        assert!(inner.is_file(), "图文件本身不在:{}", inner.display());
        assert_eq!(std::fs::read(inner).unwrap(), b"PNGDATA", "图的内容得一致");
        drop(dir);
    }

    /// **正文里的相对路径在导出后还指得到。** 图拷对了但正文里的地址变了
    /// (比如被改写成绝对路径),拷到别的机器上一一样是裂的——所以要验
    /// Markdown 里的地址**一个字都没动**
    #[test]
    fn 导出后正文里的图片地址没被改() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let id = v.save(&input("https://a.com", "甲", "![封面](assets/PLACEHOLDER/0.png)"))
            .unwrap();
        // 用真实 id 重写一遍,模拟 localize_images 之后的正文
        let real_id = v.id_of(&id.filename).unwrap();
        let path = v.clips_dir().join(&id.filename);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("PLACEHOLDER", &real_id)).unwrap();
        let img_dir = v.assets_dir().join(&real_id);
        std::fs::create_dir_all(&img_dir).unwrap();
        std::fs::write(img_dir.join("0.png"), b"PNGDATA").unwrap();

        let out = TempDir::new().unwrap();
        let report = v.export_folder(out.path(), None).unwrap();
        let md = std::fs::read_to_string(out.path().join(&report.markdown)).unwrap();

        assert!(
            md.contains(&format!("assets/{real_id}/0.png")),
            "正文里的地址得原样留着,改了到别的机器上照样裂:{}",
            md
        );
        drop(dir);
    }

    /// **只导选中那几篇,图也只带那几篇的。** 导三篇结果把整库的图都拷过去,
    /// 用户发出去一个几十兆的文件夹,里面全是他没选的东西
    #[test]
    fn 导出的图只带选中的那几篇() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文甲")).unwrap();
        let b = v.save(&input("https://b.com", "乙", "正文乙")).unwrap();
        for name in [&a.filename, &b.filename] {
            let i = v.id_of(name).unwrap();
            let d = v.assets_dir().join(&i);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("0.png"), b"X").unwrap();
        }

        let out = TempDir::new().unwrap();
        let report = v.export_folder(out.path(), Some(std::slice::from_ref(&a.filename))).unwrap();

        assert_eq!(report.images, 1, "只该带选中那篇的图");
        let n = std::fs::read_dir(out.path().join("assets")).unwrap().count();
        assert_eq!(n, 1, "只该有一个目录");
        drop(dir);
    }

    /// **往一个已经有东西的目录里导,不许删掉用户自己的文件。**
    /// 用户会拿现成的文件夹当导出位置,里面已经有他的笔记了
    #[test]
    fn 导出不删用户已有的文件() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com", "甲", "正文甲")).unwrap();

        let out = TempDir::new().unwrap();
        let mine = out.path().join("我自己的笔记.md");
        let mine_text = "别删我";
        std::fs::write(&mine, mine_text).unwrap();

        v.export_folder(out.path(), None).unwrap();

        assert_eq!(
            std::fs::read_to_string(&mine).unwrap(),
            mine_text,
            "导出动了用户的文件"
        );
        drop(dir);
    }

    /// 库里没有图时也不该报错。**没下成功的图保持原样**,那正是最常见的情况
    #[test]
    fn 没图也能导() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com", "甲", "正文甲")).unwrap();

        let out = TempDir::new().unwrap();
        let report = v.export_folder(out.path(), None).unwrap();

        assert_eq!(report.images, 0);
        assert!(out.path().join(&report.markdown).is_file(), "Markdown 得在");
        drop(dir);
    }

    /// **只导选中的那几篇。**这是本功能的全部意义:用户挑出三篇发给别人,
    /// 结果导出文件里塞了两百篇,他还得回去手工删——那还不如不导
    #[test]
    fn 只导选中的那几篇() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文甲")).unwrap();
        let _b = v.save(&input("https://b.com", "乙", "正文乙")).unwrap();
        let c = v.save(&input("https://c.com", "丙", "正文丙")).unwrap();

        let md = v.export_selected(&[a.filename.clone(), c.filename.clone()]).unwrap();

        assert!(md.contains("甲"), "选中的要导出来");
        assert!(md.contains("丙"), "选中的要导出来");
        assert!(!md.contains("乙"), "没选中的绝不能进去");
        assert!(md.contains("共 2 篇"), "篇数得对,不然用户以为全导了");
        drop(dir);
    }

    /// **选中的那几篇,标签和批注要跟着走。**用户挑这几篇多半是为了发出去
    /// 或换工具读,这两样是"这批东西是我的"的痕迹;只搬正文等于把他
    /// 整理过的东西扔了
    #[test]
    fn 导出的那几篇带着标签和批注() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let saved = v.save(&input("https://a.com", "甲", "正文甲")).unwrap();
        v.set_tags(&saved.filename, &["重要".to_string(), "待读".to_string()])
            .unwrap();
        // 写入和断言**用同一个变量**:这两行各写一遍中文的话,
        // 改文案只改一处就会红,而那个红跟功能没关系
        let note = "下周组会要讲这篇";
        v.set_note(&saved.filename,  note, None).unwrap();

        let md = v.export_selected(std::slice::from_ref(&saved.filename)).unwrap();

        assert!(md.contains("重要"), "标签没进去:{}", md);
        assert!(md.contains("待读"));
        assert!(md.contains(note), "批注没进去:{}", md);
        drop(dir);
    }

    /// **全库导出不能被选中导出的改动弄坏。**两条路共用一个渲染函数,
    /// 这条测试就是防着"改选中导出时不小心把全库导出弄坏了"
    #[test]
    fn 全库导出不受影响() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        v.save(&input("https://a.com", "甲", "正文甲")).unwrap();
        v.save(&input("https://b.com", "乙", "正文乙")).unwrap();

        let md = v.export_markdown().unwrap();

        assert!(md.contains("甲") && md.contains("乙"));
        assert!(md.contains("共 2 篇"));
        drop(dir);
    }

    /// 名单里有一个不存在的文件。**不当失败**:用户选中的东西此刻可能
    /// 正在被同步软件搬走,而那个文件他本来就有,随时能再导一次
    #[test]
    fn 名单里有不存在的文件也不失败() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文甲")).unwrap();

        let md = v
            .export_selected(&[a.filename.clone(), "压根不存在.md".to_string()])
            .unwrap();

        assert!(md.contains("甲"), "能导的那篇得导出来");
        drop(dir);
    }

    /// 隔离区里已经有同名文件时**不许覆盖**。用户从备份里把一个文件塞回
    /// 隔离区,再点一次"移到隔离区",覆盖了就凭空少了一篇——而这个操作
    /// 界面上看着像幂等的
    #[test]
    fn 隔离区已有同名文件不许覆盖() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文甲")).unwrap();
        v.trash(&a.filename).unwrap();
        v.purge(&a.filename).unwrap();

        // 造一个同名文件在隔离区里,内容不同
        let q = v.deleted_dir().join(&a.filename);
        std::fs::write(&q, "从备份塞回来的那份").unwrap();

        // 把回收站里那份重新塞回去,再点一次
        let back = v.trash_dir().join(&a.filename);
        std::fs::write(&back, "回收站里那份").unwrap();
        let err = v.purge(&a.filename).unwrap_err();

        assert!(
            matches!(err, VaultError::AlreadyExists(_)),
            "该报 AlreadyExists,实际:{:?}",
            err
        );
        assert_eq!(
            std::fs::read_to_string(&q).unwrap(),
            "从备份塞回来的那份",
            "隔离区里那份得原封不动"
        );
        drop(dir);
    }

    /// 隔离区里的也要能翻回**内容**看一眼,不然用户判断不了要不要删
    #[test]
    fn 隔离区里的能看内容() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文里有独特的一段话")).unwrap();
        v.trash(&a.filename).unwrap();
        v.purge(&a.filename).unwrap();

        let content = v.read_trash_clip(&a.filename).unwrap();

        assert!(content.body.contains("独特的一段话"), "读不出内容");
        drop(dir);
    }

    /// **到期自动清。** 不设这个的话隔离区只进不出,一年之后它就是
    /// 第二个剪藏库,用户还得自己想法子删
    #[test]
    fn 到期的隔离文件会被清掉() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "旧的", "正文")).unwrap();
        let b = v.save(&input("https://b.com", "新的", "正文")).unwrap();
        v.trash(&a.filename).unwrap();
        v.trash(&b.filename).unwrap();
        v.purge(&a.filename).unwrap();
        v.purge(&b.filename).unwrap();
        // 把 a 的时间戳推到 40 天前。**改 mtime 而不是等**:
        // 测试里等 30 天是不可能的
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 86_400);
        filetime_set(&v.deleted_dir().join(&a.filename), old);

        let removed = v.sweep_deleted().unwrap();

        assert_eq!(removed, 1, "只该清掉到期的那一个");
        assert!(!v.deleted_dir().join(&a.filename).exists(), "到期那个还在");
        assert!(v.deleted_dir().join(&b.filename).exists(), "没到期的被误删了");
        drop(dir);
    }

    /// 清空回收站也要走隔离区,不能真删
    #[test]
    fn 清空回收站是移到隔离区不是抹掉() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        let b = v.save(&input("https://b.com", "乙", "正文")).unwrap();
        v.trash(&a.filename).unwrap();
        v.trash(&b.filename).unwrap();

        v.empty_trash().unwrap();

        assert!(!v.trash_dir().join(&a.filename).exists());
        assert!(v.deleted_dir().join(&a.filename).exists(), "清空回收站把东西抹了");
        assert!(v.deleted_dir().join(&b.filename).exists(), "清空回收站把东西抹了");
        drop(dir);
    }

    /// 「清空隔离区」是真删,而且要报出清了几篇
    #[test]
    fn 清空隔离区是真的清空() {
        let (dir, v) = vault();
        v.ensure_dirs().unwrap();
        let a = v.save(&input("https://a.com", "甲", "正文")).unwrap();
        v.trash(&a.filename).unwrap();
        v.purge(&a.filename).unwrap();

        v.forget_all().unwrap();

        assert!(!v.deleted_dir().join(&a.filename).exists(), "还在");
        drop(dir);
    }

    fn filetime_set(path: &std::path::Path, t: std::time::SystemTime) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(t).unwrap();
    }

}
