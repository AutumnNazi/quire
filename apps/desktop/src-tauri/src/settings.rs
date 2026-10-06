//! 记住用户上一次的选择。
//!
//! ## 为什么必须有这个
//!
//! 原来剪藏库路径只存在内存里:用户手动指过一次目录,**每次重启都要重新指**。
//! 剪藏全在那儿,软件却装作不知道在哪。对一个 local-first 工具来说,这是
//! 最伤的一处——卡的不是某个功能,是**每一次打开**。
//!
//! 同一个文件里还记着剪贴板监控开关,它有同一个毛病:每次启动都回到关。
//!
//! **界面语言不在这儿。** 它已经存在 `localStorage` 里了(`i18n.ts` 的
//! `setLocale`)。同一个东西存两遍,迟早两边对不上——那次"我明明切了语言"
//! 查起来会很热闹。
//!
//! ## 规矩:配置坏了,最坏结果是"回到默认值"
//!
//! 这里**存的是偏好,不是数据**。剪藏在剪藏库里,配置里一个字节都没有。
//! 所以读不出来、写不进去、写成乱码,一律退回默认值继续跑:
//!
//! - 读失败 → 全部用默认值(默认目录是 `文档/Quire`,用户照样能用)
//! - 写失败 → 不报错、不打断,只在返回值里说一句
//! - 内容不对 → 整个当没这份配置
//!
//! **任何情况下都不许因为配置的问题动用户的剪藏文件。**

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 配置文件的名字。放在系统配置目录下,不跟剪藏库混在一起——
/// 剪藏库可能被用户搬到网盘上,配置不该跟着搬。
pub const FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// 上一次选的剪藏库目录。空 = 用默认目录。
    ///
    /// **存绝对路径。** 存相对路径的话,配置目录一挪(整个 home 换盘符、
    /// 用户改系统语言导致 profile 路径变)就指着别的地方去了,而用户以为
    /// 指着的是原来那个
    pub vault_path: Option<PathBuf>,
    /// 剪贴板监控开没开。默认关——关着的时候 Quire 一个字节都不读剪贴板
    pub watch_clipboard: bool,
    /// 上一次打开的是哪一篇。启动时把用户放回原处
    ///
    /// **存文件名,不存绝对路径。** 存绝对路径的话剪藏库一挪,这个值就
    /// 指向一个不存在的地方,而用户以为它还有效
    ///
    /// 记下来的那篇后来被删了怎么办?**什么都不做**:界面上没有这一篇就
    /// 回到列表,和它还在的时候一模一样。为一篇读不到的文件拦着整个
    /// 启动流程,那是拿一个边角情况换一次白屏
    #[serde(default)]
    pub last_read: Option<String>,
    /// **问过用户"要不要开剪贴板监控"没有。**
    ///
    /// 存这个而不是存"用户说了不要":两者在界面上表现得一样,但只有
    /// 前者能让他改主意之后**不再被问**——用户拒绝一次之后,不该每次
    /// 启动都被同一个问题拦住
    ///
    /// 默认 `false`,也就是**第一次启动一定会问**
    #[serde(default)]
    pub watch_asked: bool,
    /// 最近搜过的词,新的在前,最多 [`RECENT_SEARCHES_MAX`] 条。
    ///
    /// **为什么存着而不是只在内存里。** 用户搜不到的时候想不起当初搜的那个词,
    /// 这是"找不回来"最常见的一种——他记得有那篇文章,想不起它叫什么。
    /// 而"再搜一次上次搜的"这条,必须跨重启有效,不然每关一次软件就白记了
    #[serde(default)]
    pub recent_searches: Vec<String>,
}

/// 最近搜索留几条。
///
/// **10 条,不是因为 10 是个好数字,是因为再多用户也不会去翻。**
/// 这一栏是给"我刚才搜过什么"用的,不是搜索历史浏览器——
/// 摆三十条,他反而会在里面找,找的过程比重新想一遍还累
pub const RECENT_SEARCHES_MAX: usize = 10;

/// 记一次搜索。**新的排最前,重复的只留最新那条。**
///
/// 纯函数,不碰文件——写盘是命令层的事,这样才好测
pub fn push_recent_search(list: &mut Vec<String>, query: &str) {
    let q = query.trim();
    // 空串和纯空格不进列表。"搜一下"再清空框,不该在历史里留下一条空
    if q.is_empty() {
        return;
    }
    list.retain(|s| s != q);
    list.insert(0, q.to_string());
    list.truncate(RECENT_SEARCHES_MAX);
}

impl Settings {
    /// 从配置目录读一份。**读不出来就返回默认值,绝不返回 Err。**
    ///
    /// 配置里没有任何用户数据,所以"读不出来"这件事没有严重性可言——
    /// 拿默认值继续跑,顶多让用户重选一次目录。为它弹一条红条反而是惊吓
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE_NAME);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        // 解析失败同样退回默认值。**别试图"抢救"半个文件**——万一用户
        // 正在手改它,抢救出来的是半条路径,比没有更糟
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// 存一份。**写不进去就返回一句话,不抛。**
    ///
    /// 先写临时文件再改名:写到一半断电的话,留下的还是一个完整的旧配置,
    /// 而不是半截新配置
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let final_path = dir.join(FILE_NAME);
        let tmp = dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &final_path).map_err(|e| e.to_string())
    }

    /// 读的时候用了哪个剪藏库目录。空就是"用默认的"。
    pub fn vault_dir(&self) -> Option<&Path> {
        self.vault_path.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn settings(dir: &Path) -> Settings {
        Settings {
            vault_path: Some(dir.join("我的剪藏")),
            watch_clipboard: true,
            last_read: None,
            watch_asked: false,
            recent_searches: vec!["苹果".into()],
        }
    }

    /// 这是**整个模块存在的理由**:重启一次,不该要重选目录
    #[test]
    fn 存了再读之后剪藏库路径还在() {
        let d = TempDir::new().unwrap();
        settings(d.path()).save(d.path()).unwrap();
        let back = Settings::load(d.path());
        assert_eq!(back.vault_dir(), Some(d.path().join("我的剪藏").as_path()));
    }

    /// **新用户一定会被问一次"要不要开剪贴板监控"。**
    ///
    /// 这条锁的是一个产品决定而不是实现细节:剪贴板监控是 Quire 零安装
    /// 门槛的**唯一实现路径**,默认关着的话用户得自己发现工具栏上那个
    /// 小复选框,而大部分人第一次用稍后读工具卡住的就是"到底存哪儿去了"。
    /// `watch_asked` 默认 false,就是为了让那个问题一定会出现
    #[test]
    fn 新用户会被问一次要不要开监控() {
        let d = TempDir::new().unwrap();
        let s = Settings::load(d.path());
        assert!(!s.watch_asked, "第一次启动必须问,不然用户永远发现不了那个开关");
    }

    /// **问过一次之后就不再问。** 用户拒绝过一次,不该每次启动都被同一个
    /// 问题拦住——那不是引导,那是骚扰
    #[test]
    fn 问过之后记得住() {
        let d = TempDir::new().unwrap();
        let mut s = settings(d.path());
        s.watch_asked = true;
        s.save(d.path()).unwrap();
        assert!(Settings::load(d.path()).watch_asked);
    }

    /// **他自己拨了开关,也算"已经回答过"。** 用户在工具栏上开了或者关了,
    /// 那是拿行动回答了,不该再弹一次同样的问题
    #[test]
    fn 自己拨过开关也算回答过了() {
        let d = TempDir::new().unwrap();
        let mut s = settings(d.path());
        s.watch_asked = true;
        s.save(d.path()).unwrap();
        let back = Settings::load(d.path());
        assert!(back.watch_asked);
        assert!(back.watch_clipboard);
    }

    /// 旧版本写的配置里没有这个字段。**读出来要能继续跑**,
    /// 而不是因为少一个字段整个配置报废
    #[test]
    fn 老配置里没这个字段也能读() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join(FILE_NAME), r#"{"watchClipboard":true}"#).unwrap();
        let s = Settings::load(d.path());
        assert!(s.watch_clipboard, "老配置里的值别丢");
        assert!(!s.watch_asked, "老用户按新逻辑该被问一次");
    }

    /// 监控开关也得记住。用户明确开过一次,每次启动弹回"关"是在替他做决定
    #[test]
    fn 监控开关也跟着记住() {
        let d = TempDir::new().unwrap();
        settings(d.path()).save(d.path()).unwrap();
        assert!(Settings::load(d.path()).watch_clipboard);
    }

    /// 一份配置都没有的时候用默认值。**默认是"没指过目录"**,
    /// 由调用方决定落到 `文档/Quire`
    #[test]
    fn 没有配置就用默认值() {
        let d = TempDir::new().unwrap();
        let s = Settings::load(d.path());
        assert!(s.vault_dir().is_none());
        assert!(!s.watch_clipboard, "监控默认必须关着");
    }

    /// 配置是**偏好不是数据**:它坏了最多让用户重选一次,绝不能让软件起不来。
    /// 乱码文件、回退到默认值,并且不删它——用户还能自己看一眼
    #[test]
    fn 配置是乱码就退回默认值() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join(FILE_NAME), "这不是 JSON,是一段乱码").unwrap();
        let s = Settings::load(d.path());
        assert!(s.vault_dir().is_none());
        assert!(d.path().join(FILE_NAME).exists(), "别把用户的文件删了");
    }

    /// 只剩一半的 JSON(`{"vaultPath": "D:/x"`)照样能读出来那部分。
    /// 加字段不该让老用户的配置整个作废
    #[test]
    fn 配置缺字段也不作废() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join(FILE_NAME), r#"{"vaultPath":"D:/我的剪藏"}"#).unwrap();
        let s = Settings::load(d.path());
        assert_eq!(s.vault_dir(), Some(Path::new("D:/我的剪藏")));
        assert!(!s.watch_clipboard, "没写的字段用默认值");
    }

    /// 存的时候是**先临时文件再改名**。写到一半断电,留下的还是一个完整的
    /// 旧配置,而不是半截新配置——半截 JSON 读出来就是全丢
    #[test]
    fn 存完不留临时文件() {
        let d = TempDir::new().unwrap();
        settings(d.path()).save(d.path()).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件没清掉:{leftovers:?}");
    }

    /// 存两次不会把文件写坏。第一份配置不该被第二份截断
    #[test]
    fn 反复存不会写坏() {
        let d = TempDir::new().unwrap();
        settings(d.path()).save(d.path()).unwrap();
        let mut second = settings(d.path());
        second.watch_clipboard = false;
        second.save(d.path()).unwrap();
        assert!(!Settings::load(d.path()).watch_clipboard, "第二次存该盖掉第一次");
    }

    /// 目录不存在时自己建出来。第一次运行就是这种情况
    #[test]
    fn 配置目录不存在就自己建() {
        let d = TempDir::new().unwrap();
        let nested = d.path().join("a").join("b").join("c");
        settings(&nested).save(&nested).unwrap();
        assert_eq!(Settings::load(&nested), settings(&nested));
    }

    /// 存不进去要说句话,但**不能炸**。配置目录只读的时候软件照样要能开
    #[test]
    fn 存不进去就说一句而不是炸() {
        // Windows 上给目录去掉写权限不好使(ACL 会绕过去),这里改用一个
        // 一定失败的目标:把配置文件所在的路径占成一个目录
        let d = TempDir::new().unwrap();
        let blocker = d.path().join(FILE_NAME);
        std::fs::create_dir(&blocker).unwrap();
        let err = settings(d.path()).save(d.path());
        assert!(err.is_err(), "写不进去得说句话");
        assert!(!err.unwrap_err().is_empty(), "理由不能是空串");
    }

    /// 默认是"关着"。**这条是隐私承诺的落点**:配置读不出来的时候,
    /// 监控必须是关的,不能是开的
    #[test]
    fn 配置读不出来时监控保持关着() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join(FILE_NAME), "{").unwrap();
        assert!(!Settings::load(d.path()).watch_clipboard);
    }

    #[test]
    fn 上次读到哪一篇能存能读回来() {
        let d = TempDir::new().unwrap();
        let cfg = Settings {
            last_read: Some("2026-09-30-abc-example-com.md".to_string()),
            ..Default::default()
        };
        cfg.save(d.path()).unwrap();
        assert_eq!(
            Settings::load(d.path()).last_read,
            Some("2026-09-30-abc-example-com.md".to_string())
        );
    }

    /// 老版本用户的配置里没有这个键。**缺了必须还是默认值,不能反序列化失败**
    #[test]
    fn 老配置没这个键也能读() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join(FILE_NAME), r#"{"watchClipboard":true}"#).unwrap();
        let cfg = Settings::load(d.path());
        assert!(cfg.watch_clipboard);
        assert_eq!(cfg.last_read, None);
    }

    /// 存的是**文件名**,不是绝对路径。存绝对路径的话,剪藏库一挪,
    /// 这个值就成了指向不存在之处的字符串,而用户以为它还有效
    #[test]
    fn 存的是文件名不是路径() {
        let d = TempDir::new().unwrap();
        Settings {
            last_read: Some("a.md".to_string()),
            ..Default::default()
        }
        .save(d.path())
        .unwrap();
        let raw = std::fs::read_to_string(d.path().join(FILE_NAME)).unwrap();
        assert!(raw.contains("lastRead"), "键名该是 lastRead: {raw}");
        assert!(!raw.contains("\\"), "不该存反斜杠路径: {raw}");
    }

    /// **新的排最前,重复的只留最新那条。** 反复搜同一个词不该在历史里
    /// 占三行——那一行是给"我刚才搜过什么"用的,重复出现等于什么都没说
    #[test]
    fn 新的排最前重复的只留一条() {
        let mut list = vec!["甲".to_string()];
        push_recent_search(&mut list, "乙");
        assert_eq!(list, vec!["乙", "甲"]);

        push_recent_search(&mut list, "甲");
        assert_eq!(list, vec!["甲", "乙"], "重复的没被提到最前");
        assert_eq!(list.iter().filter(|s| *s == "甲").count(), 1);
    }

    /// **超出上限就把最旧的挤掉。** 这一栏摆三十条用户也不会去翻,
    /// 摆三十条他反而会在里面找,找的过程比重新想一遍还累
    #[test]
    fn 超过上限就挤掉最旧的() {
        let mut list = Vec::new();
        for i in 0..(RECENT_SEARCHES_MAX + 5) {
            push_recent_search(&mut list, &format!("词{i}"));
        }
        assert_eq!(list.len(), RECENT_SEARCHES_MAX);
        assert_eq!(list[0], format!("词{}", RECENT_SEARCHES_MAX + 4), "新的该在最前");
        assert!(
            !list.iter().any(|s| s == "词0"),
            "最旧的没被挤掉,列表会一直长"
        );
    }

    /// **空串和纯空格不进列表。** "搜一下"再把框清空,不该留下一条空
    #[test]
    fn 空的不进列表() {
        let mut list = Vec::new();
        push_recent_search(&mut list, "");
        push_recent_search(&mut list, "   ");
        assert!(list.is_empty(), "空串进了历史: {list:?}");
    }

    /// **前后空格不该跟着存。** 用户手滑多敲的那个空格会让他在
    /// 历史里看到一条看着眼熟又对不上的词
    #[test]
    fn 两头的空格去掉再记() {
        let mut list = Vec::new();
        push_recent_search(&mut list, "  苹果  ");
        assert_eq!(list, vec!["苹果"]);
    }

    /// **跨重启还在。** 这是这个功能存在的理由——"再搜一次上次搜的"
    /// 必须跨重启有效,不然每关一次软件就白记了
    #[test]
    fn 最近搜索跨重启还在() {
        let d = TempDir::new().unwrap();
        let mut cfg = settings(d.path());
        push_recent_search(&mut cfg.recent_searches, "苹果");
        push_recent_search(&mut cfg.recent_searches, "手机");
        cfg.save(d.path()).unwrap();

        let back = Settings::load(d.path());
        assert_eq!(back.recent_searches, vec!["手机", "苹果"]);
    }

    /// **老配置里没有这个键也得读得出来。** 不然用户升级上来直接读不出来,
    /// 整个配置退回默认值——**剪藏库路径也跟着没了**,那才是灾难
    #[test]
    fn 没有这个键的老配置照样读得出来() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join(FILE_NAME),
            r#"{"vaultPath":"C:/我的剪藏","watchClipboard":true}"#,
        )
        .unwrap();
        let cfg = Settings::load(d.path());
        assert_eq!(cfg.vault_path, Some(std::path::PathBuf::from("C:/我的剪藏")));
        assert!(cfg.recent_searches.is_empty(), "没有这个键");
    }
}
