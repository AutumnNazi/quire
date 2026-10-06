/**
 * 多语言文案。**中文是源语言**——key 在这里定,英文那本跟着补。
 *
 * 不引第三方 i18n 库:全应用的文案就这一百来条,一个 `t()` 加两本词典
 * 够了,为此多背一个运行时依赖不值当(README 里明写了「少一个依赖少一份
 * 供应链负担」)。
 *
 * 三条规矩,`i18n.test.ts` 逐条盯着:
 *
 * 1. **两条语言的 key 集合必须完全一致。**少一个键英文用户就看见一串
 *    `filter.unread` 这样的代号,比中文还糟。
 * 2. **占位符必须两边同名。**英文那句漏了 `{n}`,界面就变成
 *    「Marked read as  articles」,比少个键还难发现——它不报错,只是不对。
 * 3. **词典里不许有没人用的键。**改文案时顺手留下一个没人引用的旧键,
 *    下一个人会以为那功能还在,照着改一遍。
 */

import type { WireError } from "./types";

export type Locale = "zh-CN" | "en";
export const LOCALES: readonly Locale[] = ["zh-CN", "en"];

/** 语言名用它自己写,别用 `Intl.DisplayNames`——那玩意儿把 zh-CN 译成
 *  「中文(中国)」,按钮上塞不下这么长一串。 */
const NAMES: Record<Locale, string> = {
  "zh-CN": "中文",
  en: "English",
};

const CATALOG: Record<Locale, Record<string, string>> = {
  "zh-CN": {
    "lang.switch": "切换到 English",
    "lang.name": "中文",

    "toolbar.paste": "粘贴剪藏",
    "toolbar.paste.title": "把剪贴板里的内容存成 Markdown(Ctrl+V)",
    "toolbar.vaultPath.title": "剪藏目录",
    "toolbar.search.placeholder": "搜索剪藏…",
    "search.scope.label": "范围",
    "search.scope.any": "全部",
    "search.scope.title": "标题",
    "search.scope.body": "正文",
    "search.scope.tag": "标签",
    "search.scope.note": "批注",
    "search.scope.title.hint": "只看标题里有这个词的",
    "search.scope.any.hint": "标题、正文、标签、批注都搜",
    "search.scope.body.hint": "只看正文里有这个词的",
    "search.scope.tag.hint": "只看标签里有这个词的",
    "search.scope.note.hint": "只看批注里有这个词的",
    "toolbar.search.title": "搜标题和正文。中文两字就能搜(比如「苹果」)。",
    "toolbar.filters.aria": "剪藏筛选",
    "toolbar.import": "导入",
    "toolbar.import.title": "把一个文件夹里的 Markdown 导入剪藏库。只读源文件,不会改动它们。",
    "toolbar.export": "导出",
    "toolbar.export.title": "把整个剪藏库导出成一个 Markdown 文件,Obsidian / Logseq 都能直接打开",
    "toolbar.open": "打开剪藏目录",
    "toolbar.pick": "更换目录",
    "toolbar.refresh": "刷新",
    "toolbar.stats": "统计",
    "toolbar.stats.title": "看看这库里都存了些什么",
    "toolbar.refresh.title": "重新读一遍剪藏库(快捷键 F5)。在别处改过文件之后用它。",
    "toolbar.shortcut.title": "↑↓ 上下翻 · r 标已读 · a 归档 · / 搜索 · Ctrl+V 剪藏",
    "toolbar.shortcut.text": "<kbd>↑</kbd><kbd>↓</kbd> 翻 <kbd>r</kbd> 读完 <kbd>a</kbd> 归档 <kbd>/</kbd> 搜 <kbd>F5</kbd> 刷新",

    // 快捷键面板。工具栏上那行提示点开 / 按 `?` 打开
    "shortcut.title": "快捷键",
    "shortcut.close": "关闭",
    "shortcut.group.read": "翻着看",
    "shortcut.group.act": "对某篇动手",
    "shortcut.group.anywhere": "在哪儿都能按",
    "shortcut.group.typing": "正在打字的时候",
    "shortcut.next": "下一篇",
    "shortcut.prev": "上一篇",
    "shortcut.search": "搜",
    "shortcut.escape": "取消选中 / 关掉面板",
    "shortcut.read": "标已读 / 未读",
    "shortcut.archive": "归档(从列表挪走,东西还在)",
    "shortcut.selectAll": "全选列表里看得见的",
    "shortcut.clip": "把刚复制的那段存进来",
    "shortcut.refresh": "重新扫一遍剪藏库",
    "shortcut.help": "打开这张表",
    "shortcut.saveNote": "存下批注",
    "shortcut.dropNote": "丢掉这次改动,退回上次存的",
    "shortcut.confirm": "改完标题 / 标签,回车确认",

    "filter.all": "全部",
    "filter.today": "今天",
    "filter.today.title": "今天剪的。按日历日算,不是「24 小时内」——凌晨剪的那篇还归今天。",
    "filter.unread": "未读",
    "filter.unread.title": "没读过、也没归档的",
    "filter.week": "每周",
    "filter.week.title": "按自然周分组,一眼看出这周积了多少",
    "filter.starred": "收藏",
    "filter.starred.title": "你标过星的那些。**和归档不是一回事**:归档是读完挪到一边,收藏是一直留着——存到几百篇之后,这一步治的是「哪几篇值得再看一遍」。",
    "filter.archived": "归档",
    "filter.archived.title": "归档过的剪藏。归档是挪到一边,不是删掉,随时能翻回来。",
    "filter.trash": "回收站",
    "filter.trash.title": "删掉的剪藏。放回来随时能翻回原位,彻底删除就没有了。",

    "watch.label": "监控剪贴板",
    "watch.broken": "监控开着,但一直读不到剪贴板——可能是别的程序一直占着它。先关掉再打开试试",
    "watch.title": "开启后,你在别处复制文章时会自动提示存到 Quire。默认关闭。",

    "batch.count": "已选 {n} 篇",
    // 报的是**做完了几篇**,不是选中了几篇——原来这里借用 `batch.count`,
    // 于是标完已读弹出来一句"已选 3 篇",用户看不出这事到底成没成
    "batch.flagsDone": "{action} {n} 篇",
    "batch.read": "标已读",
    "batch.unread": "标未读",
    "batch.archive": "归档",
    "batch.star": "收藏",
    "batch.star.title": "给选中这几篇都标上星",
    "batch.delete": "删除",
    "batch.clear": "取消",
    "batch.tags": "标签",
    "batch.export": "导出",
    "batch.export.title": "把选中的几篇拼成一个 Markdown 文件,标签和批注一起带上",
    "batch.tagsApply": "打上这几个",
    "batch.tagsReplace": "**替换**掉选中这几篇原有的标签,不是追加。",
    "batch.tagsDone": "给 {n} 篇打了标签",

    "list.searchNone.titleWith": "没找到「{query}」",
    "list.searchNone.body":
      "换个词试试,或者从下面挑一个——你记得存过东西、想不起它叫什么,这比换十个词都快。",
    "list.searchNone.widenScope": "在全部内容里搜",
    "list.searchNone.recent": "你搜过这些",
    "list.fuzzyNotice": "没找到完全一样的。下面是差一个字就对得上的 {n} 篇,不一定是你要的,但先看看。",
    "list.fuzzyTag": "近似",
    "list.fuzzyTag.title": "这条是差一个字找出来的,不是完全对上",
    "list.unreadBadge": "{n} 篇没读",
    // 列表一次最多画 200 条。**这一条是上限能不能立住的另一半**——
    // 少了它,用户存了八百篇只看得到两百,会以为剩下的丢了
    "list.more": "还有 {n} 篇没显示",
    "list.more.action": "再显示 {n} 篇",
    "list.empty.title": "剪藏库还是空的",
    "list.empty.body": "剪藏就在两步里:",
    "firstRun.paste": "在任意文章页选中正文,复制,再回到这里按 Ctrl+V",
    "firstRun.watch": "或者开启剪贴板监控,以后复制了它会主动问你",
    "firstRun.askWatch": "开启剪贴板监控",
    "firstRun.askWatch.title":
      "开启后,你在别处复制文章时它会弹一条问你存不存。它不会自己存,也不联网。",
    "list.todayEmpty.title": "今天还没剪东西",
    "list.todayEmpty.body": "换个日期看看,或者点「全部」。",
    "list.trashEmpty.title": "回收站是空的",
    "list.trashEmpty.body": "删掉的东西会先在这儿待着。",
    "list.unreadEmpty.title": "没有未读了",
    "list.unreadEmpty.body": "都读过了。要看全部,点「全部」。",
    "list.archivedEmpty.title": "还没有归档",
    "list.archivedEmpty.body": "看完不打算再看的,在正文页点「归档」挪到这儿。",
    "list.none.title": "没有剪藏",
    "list.tagEmpty.title": "没有带「{tag}」的",
    "list.tagEmpty.body": "换一个标签,或者点一下标签栏里那个取消筛选。",
    "list.weekEmpty.title": "没有可回顾的剪藏",
    "list.weekHeader": "{year} 年第 {week} 周 — {count} 篇{suffix}",
    "list.weekUnread": " · 还有 {n} 篇没读",
    "list.weekAllRead": " · 都读完了",

    "clip.read": "已读",
    "clip.unstar.title": "取消收藏",
    "clip.markRead": "标已读",
    "clip.markUnread.title": "点一下标回未读",
    "clip.markRead.title": "读完了,点一下标已读",

    "trash.header": "{count} 篇 · 共 {size}",
    "trash.headerUnknown": "{count} 篇 · 至少 {size}({n} 篇量不出来)",
    "trash.quarantineNote": "其中 {n} 篇已隔离,{days} 天后自动清理",
    "trash.quarantinedBadge": "已隔离",
    "trash.quarantinedBadge.title": "已从回收站移走,还能放回来;{days} 天后会被自动删掉",
    "trash.sizeUnknown": "大小未知",
    "trash.empty.title": "回收站是空的",
    "trash.untitled": "读不出标题的剪藏",
    "trash.emptyTrash": "清空",
    "trash.emptyTrash.title": "处理回收站和隔离区里的内容",
    "trash.restore": "放回来",
    "trash.restore.title": "放回剪藏库的原位置,文件名和图片都跟着回来",
    "trash.purge": "移到隔离区",
    "trash.purge.title": "从回收站移走,{days} 天后自动删掉,期间还能放回来",
    "trash.purge.titleWithSize": "从回收站移走,{days} 天后自动删掉(能腾出 {size})",
    "trash.forget": "彻底删除",
    "trash.forget.title": "真删掉,磁盘上的文件也没了,没有撤销",
    "trash.forget.titleWithSize": "真删掉,磁盘上的文件也没了,没有撤销(能腾出 {size})",
    "trash.unreadable.title": "读不出内容的剪藏",
    "trash.unreadable.body":
      "这个文件的 frontmatter 坏了,可能被你手动改过。它还在回收站里,也能删掉。",

    "detail.openOriginal": "打开原文",
    "detail.rename": "标题",
    "detail.rename.title": "点一下改成你自己认得的名字",
    "detail.note": "批注",
    "detail.note.placeholder": "为什么存下这篇?读的时候想到什么?",
    "detail.note.hint": "失焦或 Ctrl+Enter 自动保存",
    "detail.progress.title": "读到哪儿了",
    "detail.archive": "归档",
    "detail.unarchive": "取消归档",
    "detail.unarchive.title": "放回全部列表",
    "detail.archive.title": "看完不打算再看的,挪到「归档」里去",
    "detail.star": "收藏",
    "detail.star.title": "标个星,提醒自己这篇还值得回头看",
    "detail.starred": "已收藏",
    "detail.unstar.title": "取消收藏",
    "detail.openFile": "打开 .md",
    "detail.openFile.title": "用系统默认程序打开这一篇",
    "detail.reveal": "在文件夹里定位",
    "detail.refetch": "补全全文",
    "detail.refetch.title":
      "当初剪藏的时候这个页面抓不到,只存了你复制的那一段。重新去抓一次把整篇补进来——标签、批注、阅读进度都不动",
    "detail.refetch.running": "在抓…",
    "detail.reveal.title": "在文件管理器里选中这一篇",
    "detail.delete": "删除",
    "detail.delete.title": "移到回收站,不是真删——放回收站里随时能捞回来",
    "detail.pickOne": "从左边选一篇",

    "tag.bar.title": "标签。点一下只看带这个标签的。",
    "tag.bar.empty": "还没有标签。打开一篇,在下面加一个。",
    "tag.bar.more": "还有 {n} 个",
    "tag.bar.more.title": "点开看全部标签",
    "tag.bar.less": "收起",
    "tag.bar.less.title": "只留用得最多的那几个",
    "tag.filter.active": "正在按「{tag}」筛选,再点一下取消",
    "tag.edit.add": "加标签",
    "tag.edit.placeholder": "敲一个,回车加上",
    "tag.edit.remove": "去掉「{tag}」",
    "tag.edit.hint": "标签存在 .md 的 frontmatter 里,可以用任何编辑器直接改。",
    "tag.count": "{n} 篇",
    "tag.menu": "改这个标签",
    "tag.rename.title": "重命名标签",
    "tag.rename.body": "把「{from}」换成什么?填一个已有的标签就是合并。",
    "tag.rename.input": "新的标签名",
    "tag.rename.action": "改",
    "tag.rename.hint": "回收站里的也跟着改。不带这个标签的一篇都不会动。",
    "tag.rename.nothing": "没有一篇带「{tag}」这个标签",
    "tag.rename.merged": "已并进「{to}」",
    "tag.rename.renamed": "已改成「{to}」",
    "error.renameTagPartial": "改了 {n} 篇,另有 {total} 篇没改成功:{detail}",
    "error.clip.openFailed": "打不开:{detail}",

    "toast.close": "关闭",
    "toast.undo": "撤销",
    "toast.saved": "已剪藏",
    "toast.savedImages": "已剪藏,{count} 张图片也存到本地了",
    "toast.savedImagesPartial": "已剪藏,存下 {count} 张图;另有 {failed} 张没存下来,还指着原网址",
    "toast.movedToTrash": "已移到回收站",
    "toast.movedMany": "已移到回收站 {n} 篇",
    "toast.restored": "放回来了",
    "toast.restoredMany": "放回来了 {n} 篇",
    "toast.progressFailed": "阅读进度没存上,关掉会丢:{detail}",
    "toast.progressRecovered": "阅读进度又存上了",
    "toast.quarantined": "已移到隔离区,{days} 天后自动删掉,期间能放回来",
    "toast.forgotten": "彻底删掉了,找不回来了",
    "toast.forgottenAll": "清空了隔离区,删掉了 {n} 篇",
    "toast.exported": "已导出",
    "toast.exportedFolder": "导出了一个文件夹,里面 {n} 张图",
    "toast.exportedFolderNoImages": "导出了。这个剪藏库里没有存到本地的图",
    "history.button": "刚才的({n})",
    "history.button.title": "剪贴板里弹出来过、但你当时没点的那些",
    "history.title": "刚才的剪藏",
    "status.title": "还没做完的",
    "status.close": "关掉",
    "status.badge": "{n} 处没做完",
    "status.badge.title":
      "有 {n} 处还没收尾:图还没下下来、正文可能没抓全、或者有文件读不出。点开看是哪几篇",
    "status.allGood": "都收好了。库里的每一篇,图都在本地,正文都在手上。",
    "status.remoteImages": "图还指着外站",
    "status.remoteImages.item": "还有 {n} 张图没下下来。导到别的机器上这些是裂图",
    "status.missingFulltext": "正文可能没抓全",
    "status.missingFulltext.item": "有地址但正文很短,多半是当初没抓到。打开它可以重试一次",
    "status.unreadable": "读不出的文件",
    "status.more": "还有 {n} 条",

    "stats.title": "这库里有什么",
    "stats.close": "关掉",
    "stats.empty": "还是空的。存几篇之后再回来看。",
    "stats.total": "总共",
    "stats.unread": "没读过",
    "stats.halfRead": "读了一半",
    "stats.withNote": "写过批注",
    "stats.byMonth": "每个月存了多少",
    "stats.monthBar": "{month}:{n} 篇",
    "stats.monthLabel": "{year} 年 {month} 月",
    "stats.bySite": "存得最多的站点",
    "stats.noSite": "(没记站点的)",
    "stats.moreSites": "还有 {n} 个站点没列出来",
    "stats.byTag": "标签",
    "stats.noTags": "还没打过标签。在详情页里加,或者左栏标签那儿右键改名。",
    "stats.moreTags": "还有 {n} 个标签没列出来",
    "stats.pickTag": "只看「{tag}」这 {n} 篇",
    "history.close": "关掉",
    "history.empty": "没有漏下的",
    "history.more": "还有 {n} 条没摆出来",
    "history.clearAll": "清空这份列表",
    "history.meta": "{when}弹出来的",
    "history.seen": " · 弹过 {n} 次",
    "export.choose": "导出成什么?",
    "export.folder": "文件夹(带图片)",
    "export.textOnly": "只要文字(单个文件)",
    "toast.imported": "导入了 {n} 篇",
    "toast.importedWithDupes": "导入了 {n} 篇,另外 {dup} 篇库里已经有了,跳过了",

    "import.choose": "从哪儿导入?",
    "import.fromFolder": "从文件夹(Markdown)",
    "import.fromFile": "从文件(Pocket / CSV / JSON)",
    "import.row": "第 {row} 行",
    "import.more": " 等 {n} 条",
    "toast.clearedTrash": "移了 {n} 篇到隔离区,{days} 天后自动清理",
    "toast.duplicate": "这篇之前剪过了",
    "toast.duplicateOpen": "打开它",
    "toast.duplicateForce": "仍然存一份",
    "toast.watchOn": "已开启监控:复制文章后会提示保存",
    "toast.detected": "检测到:{preview}",
    "toast.detectedFallback": "剪贴板内容",
    "toast.refetchDone": "整篇补上了",
    "toast.refetchEmpty": "这次抽出来是空的,原来那部分一个字没动",
    "toast.refetchFailed":
      "这个页面还是抓不到(可能要登录、纯 JS 渲染,或者被拦了)。原来那部分一个字都没少",
    "toast.save": "保存",
    "toast.ignore": "忽略",
    "toast.conflict": "这个文件在你编辑期间被别处改过,没敢写",
    "toast.conflict.discard": "读最新的,我的改动不要了",
    "toast.conflict.overwrite": "用我这份覆盖回去",
    // 批量改动。**"撤销"按钮是这一条存在的理由**——批量操作一次动十几篇,
    // 没有退路的话用户只能靠"下次少勾几篇"给自己上保险
    "toast.batchDone": "{done},撤销还在",
    "toast.undone": "退回原样了,{n} 篇",

    "confirm.batchDelete": "把选中的 {n} 篇移到回收站?",
    "confirm.batchDeleteAction": "移到回收站",
    "confirm.emptyTrash": "把回收站里的 {n} 篇移到隔离区?{days} 天后自动清理,期间能放回来",
    "confirm.emptyTrashAction": "移到隔离区",
    "confirm.emptyBoth":
      "回收站里 {trash} 篇,隔离区里 {quarantine} 篇。要移走,还是彻底删掉?",
    "confirm.quarantineAll": "全部移到隔离区",
    "confirm.forgetAll": "清空隔离区",
    "confirm.forgetAllBody": "彻底删掉隔离区里的 {n} 篇?磁盘上的文件也会没,没有撤销。",
    "confirm.forget": "彻底删掉这一篇?磁盘上的文件也会没,没有撤销。",

    "error.readFailed": "读不出来:{detail}",
    "error.starFailed": "收藏没改成:{detail}",
    "error.markReadFailed": "标已读失败:{detail}",
    "error.archiveFailed": "归档失败:{detail}",
    "error.unarchiveFailed": "取消归档失败:{detail}",
    "error.trashFailed": "删除失败:{detail}",
    "error.restoreFailed": "放回失败:{detail}",
    "error.undoFailed": "撤销失败,文件还在回收站里:{detail}",
    "error.undoPartial": "放回来 {n} 篇,但有 {missed} 篇没放回来:{reason}",
    "error.purgeFailed": "移到隔离区失败:{detail}",
    "error.searchFailed": "搜不了:{detail}",
    "error.searchSkipped": "搜不到 {n} 个文件(读不出或不是 Quire 存的剪藏):{names}",
    "error.listFailed": "列不出剪藏:{detail}",
    "error.trashUnreadable": "回收站读不出来:{detail}",
    "error.emptyTrashFailed": "移到隔离区失败:{detail}",
    "error.forgetAllFailed": "清空隔离区失败:{detail}",
    "error.forgetAllPartial": "删掉了 {ok} 篇,但有 {n} 篇没删掉:{names}",
    "error.internal": "出了点问题:{detail}",
    "error.fetch.notHttp": "这个地址抓不了:{detail}",
    "error.fetch.notHtml": "那不是一篇网页:{detail}",
    "error.fetch.timeout": "抓这个页面超时了",
    "error.fetch.tooLarge": "页面太大了,没抓:{detail}",
    "error.fetch.failed": "抓不到这个页面:{detail}",
    "error.batchFailed": "{action}失败:{detail}",
    "error.batchTagsFailed": "批量打标签失败:{detail}",
    "error.renameFailed": "标题没改成:{detail}",
    "error.noteFailed": "批注没存上:{detail}",
    "error.conflict":
      "这个文件在你编辑期间被别处改过,已经先停下、没写进去。选一个:读最新的(丢掉你刚写的),或者用你这份盖回去。",
    "error.unexpected": "出了个没接住的错:{detail}",
    "error.importFailed": "导入失败:{detail}",
    "error.importDataFailed": "从文件导入失败:{detail}",
    "error.import.readFailed": "文件读不出来:{detail}",
    "error.import.badEncoding": "这个文件不是 UTF-8 的,读不出里面的中文",
    "error.import.badFormat": "认不出这个文件的格式:{detail}",
    "error.importDataPartial": "导入了 {ok} 篇,但有 {n} 条读不出来({names})",
    "error.importAllDuplicate": "导的 {n} 篇库里都有了,一篇新的也没进来:{name}",
    "error.exportFailed": "导出失败:{detail}",
    "error.pickVaultFailed": "更换目录失败:{detail}",
    "error.clipboardEmpty": "剪贴板是空的,先在别处复制点内容",
    "error.clipboardNoContent": "剪贴板里没有可保存的内容",
    "error.unreadableFiles": "有 {n} 个文件读不出元数据:{names}",
    "error.vaultUnknown": "剪藏目录未知",
    "error.batchPartial": "{done},但有 {n} 篇没成功:{reason}",
    "error.trashPartial": "有 {n} 篇没移到回收站:{reason}",
    "error.emptyTrashPartial": "移了 {ok} 篇,但有 {n} 篇没移走:{names}",
    "error.importNone": "一篇都没导进来。{n} 个文件被跳过,比如 {name}:{reason}",
    "error.importNoneEmpty": "那个文件里没有能导的记录",
    "error.importDirsUnreadable": "有 {n} 个目录压根没读进去,里面的笔记一个都没导进来:{names}",
    "error.importPartial": "导入了 {ok} 篇,跳过 {n} 个:{names}",

    "error.vault.io": "读写剪藏文件失败:{detail}",
    "error.vault.unsafeFilename": "文件名不合法,已拒绝:{detail}",
    "error.vault.emptyContent": "剪藏内容为空,已拒绝",
    "error.vault.emptyTitle": "标题不能留空",
    "error.vault.nothingToChange": "既没说改已读,也没说改归档",
    "error.vault.notFound": "剪藏不存在:{detail}",
    "error.vault.alreadyExists": "已经有同名剪藏了,没敢放回去:{detail}",
    "error.vault.trashFull": "回收站里挤不下了,请自己清一清:{detail}",
    "error.vault.importSkippedSelf": "这就是剪藏库自己,不用导",
    "error.vault.importSkippedDuplicate": "已经在库里了:{detail}",
    "error.vault.unreadable": "剪藏已放回,但读不出元数据:{name}({detail})",
    "error.vault.poisoned": "内部状态异常,请重启 Quire",
    "error.vault.missing": "上次用的剪藏库目录不在了:{path}。剪藏多半还在那儿——把硬盘接回去,或者把目录指回去。",
    "error.vaultUnavailable": "打不开剪藏库:{detail}",
    "error.vault.badTag": "这个标签名用不了:{detail}",
    "error.vault.noFrontmatter": "读不出 frontmatter,没改:{detail}",
    // 后端只在**指纹对不上**时报这个,不是笼统的写失败。用户据此知道
    // 自己刚敲的字还在输入框里,没被冲掉
    "error.vault.changedElsewhere":
      "这个文件在你编辑期间被别处改过,没敢写。你刚敲的字还在输入框里",
    "error.vault.tooManyTags": "标签太多了,一篇最多 {detail} 个",
    "error.tagFailed": "标签没存上:{detail}",
    "error.tagsUnavailable": "标签栏拉不出来,已清空:{detail}",

    "error.app.cancelled": "操作已取消",
    "error.app.internal": "出了点岔子:{detail}",
    "error.app.pickerFailed": "文件夹选择器没响应,请再试一次",
    "error.clipboard.unavailable": "读不到剪贴板内容(可能被其他程序占用,或当前平台尚未支持)",
    "error.export.badPath": "选中的不是一个可写的文件位置",
    "error.export.writeFailed": "写不出文件:{detail}",
    "error.export.openFolderFailed": "打不开剪藏目录:{detail}",
  },

  en: {
    "lang.switch": "Switch to 中文",
    "lang.name": "English",

    "toolbar.paste": "Paste & save",
    "toolbar.paste.title": "Save whatever is on the clipboard as Markdown (Ctrl+V)",
    "toolbar.vaultPath.title": "Library folder",
    "toolbar.search.placeholder": "Search…",
    "toolbar.search.title": "Searches titles and body text. Two CJK characters are enough.",
    "search.scope.label": "Scope",
    "search.scope.any": "All",
    "search.scope.title": "Title",
    "search.scope.body": "Body",
    "search.scope.tag": "Tags",
    "search.scope.note": "Notes",
    "search.scope.title.hint": "Only clippings whose title contains this",
    "search.scope.any.hint": "Search titles, body, tags and notes",
    "search.scope.body.hint": "Only clippings whose body contains this",
    "search.scope.tag.hint": "Only clippings carrying a matching tag",
    "search.scope.note.hint": "Only clippings whose note contains this",
    "toolbar.filters.aria": "Filter clippings",
    "toolbar.import": "Import",
    "toolbar.import.title":
      "Import Markdown files from a folder. Source files are only read, never modified.",
    "toolbar.export": "Export",
    "toolbar.export.title":
      "Export the whole library as one Markdown file. Obsidian and Logseq open it directly.",
    "toolbar.open": "Open folder",
    "toolbar.pick": "Change folder",
    "toolbar.refresh": "Refresh",
    "toolbar.stats": "Stats",
    "toolbar.stats.title": "See what is actually in this library",
    "toolbar.refresh.title": "Re-read the clipping vault (F5). Use it after editing files elsewhere.",
    "toolbar.shortcut.title": "↑↓ move · r read · a archive · / search · Ctrl+V clip",
    "toolbar.shortcut.text": "<kbd>↑</kbd><kbd>↓</kbd> move <kbd>r</kbd> read <kbd>a</kbd> archive <kbd>/</kbd> search <kbd>F5</kbd> refresh",

    "shortcut.title": "Keyboard shortcuts",
    "shortcut.close": "Close",
    "shortcut.group.read": "Reading through",
    "shortcut.group.act": "Acting on one",
    "shortcut.group.anywhere": "Works anywhere",
    "shortcut.group.typing": "While typing",
    "shortcut.next": "Next",
    "shortcut.prev": "Previous",
    "shortcut.search": "Search",
    "shortcut.escape": "Clear selection / close a panel",
    "shortcut.read": "Mark read / unread",
    "shortcut.archive": "Archive (moves it aside, keeps the file)",
    "shortcut.selectAll": "Select everything listed",
    "shortcut.clip": "Save what you just copied",
    "shortcut.refresh": "Rescan the library",
    "shortcut.help": "Open this list",
    "shortcut.saveNote": "Save the note",
    "shortcut.dropNote": "Discard this edit, back to the saved one",
    "shortcut.confirm": "Confirm a title / tag edit",

    "filter.all": "All",
    "filter.today": "Today",
    "filter.today.title":
      "Clipped today. By calendar day, not 'within 24 hours' — one you clipped at 1am still counts.",
    "filter.unread": "Unread",
    "filter.unread.title": "Never read, never archived",
    "filter.week": "Weekly",
    "filter.week.title": "Grouped by calendar week, so you can see how much piled up this week",
    "filter.starred": "Starred",
    "filter.starred.title": "The ones you marked. **Not the same as archived**: archiving moves finished items aside, starring keeps something you still want. Once you have a few hundred, this is how you find the ones worth re-reading.",
    "filter.archived": "Archived",
    "filter.archived.title": "Archived clippings. Archiving moves them aside, it never deletes them.",
    "filter.trash": "Trash",
    "filter.trash.title": "Deleted clippings. Restoring puts them back exactly where they were.",

    "watch.label": "Watch clipboard",
    "watch.broken": "Watching is on but the clipboard keeps coming back unreadable — something else may be holding it. Try switching it off and on again",
    "watch.title":
      "When on, copying an article elsewhere prompts you to save it to Quire. Off by default.",

    "batch.count": "{n} selected",
    "batch.flagsDone": "{action}: {n}",
    "batch.read": "Mark read",
    "batch.unread": "Mark unread",
    "batch.archive": "Archive",
    "batch.star": "Star",
    "batch.star.title": "Star all the selected ones",
    "batch.delete": "Delete",
    "batch.clear": "Cancel",
    "batch.tags": "Tags",
    "batch.export": "Export",
    "batch.export.title":
      "Put the selected clippings into one Markdown file, tags and notes included",
    "batch.tagsApply": "Apply tags",
    "batch.tagsReplace": "This **replaces** the tags on the selected clippings, it does not add to them.",
    "batch.tagsDone": "Tagged {n}",

    "list.searchNone.titleWith": "Nothing matches “{query}”",
    "list.searchNone.body":
      "Try another word, or pick one of these below — you remember saving something, just not what it was called. This beats guessing ten more words.",
    "list.searchNone.widenScope": "Search everything",
    "list.searchNone.recent": "You searched for",
    "list.fuzzyNotice":
      "No exact match. Below are {n} that differ by one character. Not necessarily what you meant, but worth a look.",
    "list.fuzzyTag": "close",
    "list.fuzzyTag.title": "Found with one character off, not an exact match",
    "list.unreadBadge": "{n} unread",
    "list.more": "{n} more not shown",
    "list.more.action": "Show {n} more",
    "list.empty.title": "Your library is empty",
    "list.empty.body": "Saving takes two steps:",
    "firstRun.paste": "Select text on any article, copy it, then come back and press Ctrl+V",
    "firstRun.watch": "Or turn on clipboard watching, and it will offer to save from now on",
    "firstRun.askWatch": "Watch my clipboard",
    "firstRun.askWatch.title":
      "When you copy an article anywhere else, Quire asks whether to save it. It never saves on its own, and it does not go online.",
    "list.todayEmpty.title": "Nothing clipped today",
    "list.todayEmpty.body": "Try another day, or switch to “All”.",
    "list.trashEmpty.title": "The trash is empty",
    "list.trashEmpty.body": "Anything you delete waits here first.",
    "list.unreadEmpty.title": "Nothing unread",
    "list.unreadEmpty.body": "You are all caught up. Switch to “All” to see everything.",
    "list.archivedEmpty.title": "Nothing archived yet",
    "list.archivedEmpty.body": "Anything you are done with, press “Archive” on it to move it here.",
    "list.none.title": "No clippings",
    "list.tagEmpty.title": "Nothing tagged “{tag}”",
    "list.tagEmpty.body": "Try another tag, or click that tag in the bar to clear the filter.",
    "list.weekEmpty.title": "Nothing to review",
    "list.weekHeader": "Week {week} of {year} — {count}{suffix}",
    "list.weekUnread": " · {n} still unread",
    "list.weekAllRead": " · all read",

    "clip.read": "Read",
    "clip.unstar.title": "Remove the star",
    "clip.markRead": "Mark read",
    "clip.markUnread.title": "Click to mark unread again",
    "clip.markRead.title": "Finished reading, click to mark it read",

    "trash.header": "{count} items · {size}",
    "trash.headerUnknown": "{count} items · at least {size} ({n} could not be measured)",
    "trash.quarantineNote": "{n} of them quarantined, cleaned up after {days} days",
    "trash.quarantinedBadge": "Quarantined",
    "trash.quarantinedBadge.title":
      "Moved out of the trash. You can still restore it; it is deleted after {days} days.",
    "trash.sizeUnknown": "size unknown",
    "trash.empty.title": "The trash is empty",
    "trash.untitled": "Clipping with no readable title",
    "trash.emptyTrash": "Empty trash",
    "trash.emptyTrash.title": "Deal with what is in the trash and the quarantine",
    "trash.restore": "Restore",
    "trash.restore.title": "Put it back where it was, filename and images included",
    "trash.purge": "Move to quarantine",
    "trash.purge.title": "Move out of the trash, deleted after {days} days, restorable until then",
    "trash.purge.titleWithSize":
      "Move out of the trash, deleted after {days} days (frees up {size})",
    "trash.forget": "Delete permanently",
    "trash.forget.title": "Really deleted, gone from disk, no undo",
    "trash.forget.titleWithSize":
      "Really deleted, gone from disk, no undo (frees up {size})",
    "trash.unreadable.title": "Clipping with no readable content",
    "trash.unreadable.body":
      "This file has broken frontmatter, probably hand-edited. It is still in the trash, and it can still be deleted.",

    "detail.openOriginal": "Open original",
    "detail.rename": "Title",
    "detail.rename.title": "Click to rename it something you will recognise",
    "detail.note": "Note",
    "detail.note.placeholder": "Why are you keeping this? What did it make you think?",
    "detail.note.hint": "Saves when you click away, or on Ctrl+Enter",
    "detail.progress.title": "How far you got",
    "detail.archive": "Archive",
    "detail.unarchive": "Unarchive",
    "detail.unarchive.title": "Put it back in the main list",
    "detail.archive.title": "Done with it, not keeping it around? Move it to Archived.",
    "detail.star": "Star",
    "detail.star.title": "Mark it, so you remember this one is worth coming back to",
    "detail.starred": "Starred",
    "detail.unstar.title": "Remove the star",
    "detail.openFile": "Open .md",
    "detail.openFile.title": "Open this clipping with the default app",
    "detail.reveal": "Show in folder",
    "detail.refetch": "Fetch full text",
    "detail.refetch.title":
      "This page could not be fetched when you clipped it, so only the part you copied was saved. Fetch it again and merge the whole article in — tags, note and reading progress stay untouched.",
    "detail.refetch.running": "Fetching…",
    "detail.reveal.title": "Select this file in the file manager",
    "detail.delete": "Delete",
    "detail.delete.title": "Moved to the trash, not really deleted — you can fish it out any time",
    "detail.pickOne": "Pick one on the left",

    "tag.bar.title": "Tags. Click one to see only what carries it.",
    "tag.bar.empty": "No tags yet. Open a clipping and add one below.",
    "tag.bar.more": "{n} more",
    "tag.bar.more.title": "Show all tags",
    "tag.bar.less": "Show less",
    "tag.bar.less.title": "Back to the most-used tags only",
    "tag.filter.active": "Filtering by “{tag}”, click again to clear",
    "tag.edit.add": "Add tag",
    "tag.edit.placeholder": "Type one, press Enter",
    "tag.edit.remove": "Remove “{tag}”",
    "tag.edit.hint": "Tags live in the .md frontmatter, so any editor can change them too.",
    "tag.count": "{n}",
    "tag.menu": "Edit this tag",
    "tag.rename.title": "Rename tag",
    "tag.rename.body": "Replace “{from}” with what? Picking an existing tag merges them.",
    "tag.rename.input": "New tag name",
    "tag.rename.action": "Rename",
    "tag.rename.hint": "Clippings in the trash are renamed too. Files without this tag are left alone.",
    "tag.rename.nothing": "Nothing carries the tag “{tag}”",
    "tag.rename.merged": "Merged into “{to}”",
    "tag.rename.renamed": "Renamed to “{to}”",
    "error.renameTagPartial": "{n} renamed, but {total} could not be: {detail}",
    "error.clip.openFailed": "Could not open: {detail}",

    "toast.close": "Close",
    "toast.undo": "Undo",
    "toast.saved": "Saved",
    "toast.savedImages": "Saved, and {count} images stored locally too",
    "toast.savedImagesPartial": "Saved; {count} images stored locally, but {failed} could not be and still point at the original site",
    "toast.movedToTrash": "Moved to trash",
    "toast.movedMany": "Moved {n} to trash",
    "toast.restored": "Restored",
    "toast.restoredMany": "Restored {n}",
    "toast.progressFailed": "Reading position was not saved; it will be lost: {detail}",
    "toast.progressRecovered": "Reading position is being saved again",
    "toast.quarantined": "Moved to quarantine, deleted after {days} days, restorable until then",
    "toast.forgotten": "Deleted permanently, no way back",
    "toast.forgottenAll": "Emptied the quarantine, deleted {n}",
    "toast.exported": "Exported",
    "toast.exportedFolder": "Exported a folder with {n} images",
    "toast.exportedFolderNoImages": "Exported. Nothing in this library has images saved locally.",
    "history.button": "Recent ({n})",
    "history.button.title": "Things the clipboard offered that you did not save then",
    "history.title": "Recently offered",
    "status.title": "Not finished yet",
    "status.close": "Close",
    "status.badge": "{n} unfinished",
    "status.badge.title":
      "{n} things are not settled yet: images not downloaded, full text possibly not fetched, or files that cannot be read. Click to see which",
    "status.allGood": "All settled. Every clipping in here has its images locally and its full text on hand.",
    "status.remoteImages": "Images still pointing off-site",
    "status.remoteImages.item":
      "{n} images were not downloaded. These will be broken images on any other machine",
    "status.missingFulltext": "Full text possibly not fetched",
    "status.missingFulltext.item":
      "It has an address but the body is very short, so it probably failed to fetch back then. Open it to try again",
    "status.unreadable": "Files that cannot be read",
    "status.more": "{n} more",

    "stats.title": "What is in this library",
    "stats.close": "Close",
    "stats.empty": "Still empty. Save a few things and come back.",
    "stats.total": "Total",
    "stats.unread": "Unread",
    "stats.halfRead": "Half-read",
    "stats.withNote": "With notes",
    "stats.byMonth": "Saved per month",
    "stats.monthBar": "{month}: {n}",
    "stats.monthLabel": "{year}-{month}",
    "stats.bySite": "Sites you save from most",
    "stats.noSite": "(no site recorded)",
    "stats.moreSites": "{n} more sites not listed",
    "stats.byTag": "Tags",
    "stats.noTags": "No tags yet. Add them on the detail page, or right-click one in the sidebar to rename.",
    "stats.moreTags": "{n} more tags not listed",
    "stats.pickTag": "Show only these {n} tagged “{tag}”",
    "history.close": "Close",
    "history.empty": "Nothing was missed",
    "history.more": "{n} more not shown",
    "history.clearAll": "Clear this list",
    "history.meta": "Offered {when}",
    "history.seen": " · offered {n} times",
    "export.choose": "Export as",
    "export.folder": "A folder (with images)",
    "export.textOnly": "Text only (one file)",
    "toast.imported": "Imported {n}",
    "toast.importedWithDupes": "Imported {n}, skipped {dup} that were already here",

    "import.choose": "Import from",
    "import.fromFolder": "A folder (Markdown)",
    "import.fromFile": "A file (Pocket / CSV / JSON)",
    "import.row": "row {row}",
    "import.more": " and {n} more",
    "toast.clearedTrash": "Moved {n} to quarantine, cleaned up after {days} days",
    "toast.duplicate": "You clipped this before",
    "toast.duplicateOpen": "Open it",
    "toast.duplicateForce": "Save another copy",
    "toast.watchOn": "Clipboard watching on: copying an article will prompt you",
    "toast.detected": "Copied: {preview}",
    "toast.detectedFallback": "clipboard content",
    "toast.refetchDone": "Full text merged in",
    "toast.refetchEmpty": "Nothing came out of this page this time — what you already had is untouched",
    "toast.refetchFailed":
      "Still cannot fetch this page (it may need a login, be JS-rendered, or be blocking us). Nothing you already had was lost",
    "toast.save": "Save",
    "toast.ignore": "Ignore",
    "toast.conflict": "This file was changed elsewhere while you were editing it — nothing was written",
    "toast.conflict.discard": "Load the latest, drop my changes",
    "toast.conflict.overwrite": "Overwrite with mine",
    "toast.batchDone": "{done} — undo is right here",
    "toast.undone": "Put back the way it was: {n}",

    "confirm.batchDelete": "Move {n} selected to the trash?",
    "confirm.batchDeleteAction": "Move to trash",
    "confirm.emptyTrash":
      "Move {n} from the trash to quarantine? They are deleted after {days} days, restorable until then.",
    "confirm.emptyTrashAction": "Move to quarantine",
    "confirm.emptyBoth":
      "{trash} in the trash, {quarantine} in quarantine. Move them out, or delete them for good?",
    "confirm.quarantineAll": "Move all to quarantine",
    "confirm.forgetAll": "Empty quarantine",
    "confirm.forgetAllBody":
      "Delete {n} from quarantine for good? They are gone from disk too, and there is no undo.",
    "confirm.forget":
      "Delete this one for good? It is gone from disk too, and there is no undo.",

    "error.readFailed": "Could not read it: {detail}",
    "error.starFailed": "Could not update the star: {detail}",
    "error.markReadFailed": "Could not mark as read: {detail}",
    "error.archiveFailed": "Could not archive: {detail}",
    "error.unarchiveFailed": "Could not unarchive: {detail}",
    "error.trashFailed": "Could not delete: {detail}",
    "error.restoreFailed": "Could not restore: {detail}",
    "error.undoFailed": "Undo failed, the file is still in the trash: {detail}",
    "error.undoPartial": "Restored {n}, but {missed} could not be restored: {reason}",
    "error.purgeFailed": "Could not move to quarantine: {detail}",
    "error.searchFailed": "Search failed: {detail}",
    "error.searchSkipped": "Could not search {n} files (unreadable, or not saved by Quire): {names}",
    "error.listFailed": "Could not list your library: {detail}",
    "error.trashUnreadable": "Could not read the trash: {detail}",
    "error.emptyTrashFailed": "Could not move to quarantine: {detail}",
    "error.forgetAllFailed": "Could not empty the quarantine: {detail}",
    "error.forgetAllPartial":
      "Deleted {ok}, but {n} could not be deleted: {names}",
    "error.internal": "Something went wrong: {detail}",
    "error.fetch.notHttp": "That address cannot be fetched: {detail}",
    "error.fetch.notHtml": "That is not a web page: {detail}",
    "error.fetch.timeout": "Timed out fetching that page",
    "error.fetch.tooLarge": "That page is too large to fetch: {detail}",
    "error.fetch.failed": "Could not fetch that page: {detail}",
    "error.batchFailed": "{action} failed: {detail}",
    "error.batchTagsFailed": "Could not tag them: {detail}",
    "error.renameFailed": "Could not rename it: {detail}",
    "error.noteFailed": "The note did not save: {detail}",
    "error.conflict":
      "This file was changed elsewhere while you were editing it, so nothing was written. Pick one: load the latest (dropping what you just typed), or overwrite with yours.",
    "error.unexpected": "Something went wrong and nothing caught it: {detail}",
    "error.importFailed": "Import failed: {detail}",
    "error.importDataFailed": "Import from file failed: {detail}",
    "error.import.readFailed": "Could not read the file: {detail}",
    "error.import.badEncoding": "This file is not UTF-8, so the text in it cannot be read",
    "error.import.badFormat": "Unrecognised file format: {detail}",
    "error.importDataPartial":
      "Imported {ok}, but {n} could not be read ({names})",
    "error.importAllDuplicate":
      "All {n} were already in the library, so nothing new came in: {name}",
    "error.exportFailed": "Export failed: {detail}",
    "error.pickVaultFailed": "Could not change the folder: {detail}",
    "error.clipboardEmpty": "The clipboard is empty. Copy something first.",
    "error.clipboardNoContent": "There is nothing on the clipboard worth saving.",
    "error.unreadableFiles": "{n} files have unreadable metadata: {names}",
    "error.vaultUnknown": "Library folder unknown",
    "error.batchPartial": "{done}, but {n} did not work: {reason}",
    "error.trashPartial": "{n} could not be moved to the trash: {reason}",
    "error.emptyTrashPartial":
      "Moved {ok}, but {n} could not be moved: {names}",
    "error.importNone": "Nothing was imported. {n} files were skipped, for example {name}: {reason}",
    "error.importNoneEmpty": "That file has no importable records",
    "error.importDirsUnreadable": "{n} folders could not be read at all — none of the notes inside were imported: {names}",
    "error.importPartial": "Imported {ok}, skipped {n}: {names}",

    "error.vault.io": "Could not read or write a clipping file: {detail}",
    "error.vault.unsafeFilename": "That filename is not allowed, refusing: {detail}",
    "error.vault.emptyContent": "The clipping is empty, refusing",
    "error.vault.emptyTitle": "A title cannot be blank",
    "error.vault.nothingToChange": "Neither read nor archived was asked for, so nothing changed",
    "error.vault.notFound": "No such clipping: {detail}",
    "error.vault.alreadyExists": "A clipping with that name already exists, did not overwrite: {detail}",
    "error.vault.trashFull": "The trash is full, empty it yourself: {detail}",
    "error.vault.importSkippedSelf": "That is your own library, nothing to import",
    "error.vault.importSkippedDuplicate": "Already in your library: {detail}",
    "error.vault.unreadable": "Restored, but the metadata could not be read: {name} ({detail})",
    "error.vault.poisoned": "Internal state is broken, please restart Quire",
    "error.vault.missing": "The clipping folder you used last time is gone: {path}. Your clips are most likely still there — reconnect the drive, or point at it again.",
    "error.vaultUnavailable": "Cannot open the clipping vault: {detail}",
    "error.vault.badTag": "That tag name is not usable: {detail}",
    "error.vault.noFrontmatter": "No frontmatter to update, left alone: {detail}",
    "error.vault.changedElsewhere":
      "This file was changed elsewhere while you were editing it, so nothing was written. What you just typed is still in the box",
    "error.vault.tooManyTags": "Too many tags, at most {detail} per clipping",
    "error.tagFailed": "Could not save the tags: {detail}",
    "error.tagsUnavailable": "Could not load the tag bar, so it is now empty: {detail}",

    "error.app.cancelled": "Cancelled",
    "error.app.internal": "Something went wrong: {detail}",
    "error.app.pickerFailed": "The folder picker did not respond, try again",
    "error.clipboard.unavailable":
      "Could not read the clipboard (another program may be holding it, or this platform is not supported yet)",
    "error.export.badPath": "That is not a writable file location",
    "error.export.writeFailed": "Could not write the file: {detail}",
    "error.export.openFolderFailed": "Could not open the library folder: {detail}",
  },
};

const STORAGE_KEY = "quire.locale";

/** 系统语言 → 我们的语言。
 *
 *  **按偏好顺序取第一个认得的,不是"列表里有没有中文"。** `navigator.languages`
 *  是用户排过序的:系统首选英文、第二顺位中文,那就该给英文——越过他的
 *  第一选择去满足第二选择,是替用户做主。
 *
 *  认不出的语言(德语、法语…)不拦着列表继续找,只在整条列表都认不出时
 *  才落到英文:一个没翻成德语的软件,给英文也比给中文强。 */
export function resolveLocale(navigatorLanguages: readonly string[]): Locale {
  for (const tag of navigatorLanguages) {
    const lower = tag.toLowerCase();
    if (lower.startsWith("zh")) return "zh-CN";
    if (lower.startsWith("en")) return "en";
  }
  return "en";
}

function readStored(): Locale | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return raw === "zh-CN" || raw === "en" ? raw : null;
  } catch {
    // WebView 关了本地存储(无痕模式之类)。读不到就按系统语言来,别把
    // 用户卡在界面上
    return null;
  }
}

function resolveInitial(): Locale {
  const stored = readStored();
  if (stored) return stored;
  const langs = window.navigator.languages?.length
    ? window.navigator.languages
    : [window.navigator.language ?? ""];
  return resolveLocale(langs);
}

let current: Locale = resolveInitial();

const listeners = new Set<(locale: Locale) => void>();

export function getLocale(): Locale {
  return current;
}

export function localeName(locale: Locale = current): string {
  return NAMES[locale];
}

/** 切语言。**存进本地存储**:用户的选择不该每次开软件都要重来一遍。 */
export function setLocale(locale: Locale): void {
  if (locale === current) return;
  current = locale;
  try {
    window.localStorage.setItem(STORAGE_KEY, locale);
  } catch {
    // 存不下就只在这次会话里生效,不该因此报错
  }
  for (const fn of listeners) fn(locale);
}

export function onLocaleChange(fn: (locale: Locale) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

const PLACEHOLDER = /\{(\w+)\}/g;

/** 取一条文案。**参数缺了就把整个 `{xxx}` 留在原处**——
 *  悄悄换成空串的话,「导入了  篇」比报错更难让人反应过来哪里错了。 */
export function t(key: string, params: Record<string, string | number> = {}): string {
  const table = CATALOG[current];
  const template = table[key] ?? CATALOG["zh-CN"][key];
  if (template === undefined) {
    // 词典里没有这个键。`i18n.test.ts` 会让这种情况过不了 CI,
    // 走到这儿说明是运行期传了个手写错的键
    return key;
  }
  return template.replace(PLACEHOLDER, (whole, name: string) => {
    const value = params[name];
    return value === undefined ? whole : String(value);
  });
}

/** 界面是语言无关的 DOM 结构 + 会变的文案,所以切语言要**整片重画**。
 *  靠 `data-i18n` 标在元素上,新加文案的人不用记得回来登记。 */
export function applyI18n(root: ParentNode): void {
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n]")) {
    node.textContent = t(node.dataset.i18n ?? "");
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-html]")) {
    // 只有词典里的静态片段走 innerHTML,用户数据一律走 textContent
    node.innerHTML = t(node.dataset.i18nHtml ?? "");
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-title]")) {
    node.title = t(node.dataset.i18nTitle ?? "");
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-aria]")) {
    node.setAttribute("aria-label", t(node.dataset.i18nAria ?? ""));
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-placeholder]")) {
    node.setAttribute("placeholder", t(node.dataset.i18nPlaceholder ?? ""));
  }
}

/** 切到另一个语言。现在只有中英两本,所以"另一个"就是全部。 */
export function otherLocale(locale: Locale = current): Locale {
  return locale === "zh-CN" ? "en" : "zh-CN";
}

/** 认出后端送回来的错误。
 *
 *  `Array.isArray` 也要查:数组也是 `object`,光查 `typeof` 会把
 *  `["a"]` 当成错误,取出来的 `code` 是 `undefined`。 */
export function isWireError(
  value: unknown,
): value is { code: string; args?: Record<string, string> } {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    typeof (value as { code?: unknown }).code === "string"
  );
}

/** 把后端送回来的东西翻成一句人话。
 *
 *  接 `unknown` 而不是窄类型:命令的 `catch` 拿到的本来就是 `unknown`,
 *  让每个调用点自己判一遍类型,总有一处会写成 `String(err)` ——那对新的
 *  错误对象会打出 "[object Object]"。
 *
 *  **查不到代号就把代号原样返回**,绝不返回空串:空串在界面上是一片空白,
 *  用户只能对着屏幕猜软件是不是坏了,而一个看得见的 `vault.notFound`
 *  至少能让人报出"我这儿显示了一串英文代号"。 */
export function wireText(error: unknown): string {
  if (isWireError(error)) return t(`error.${error.code}`, error.args ?? {});
  if (error instanceof Error) return error.message;
  return String(error);
}

/** 把任意抛出物整理成一条能记账的 `WireError`。
 *
 *  批量撤销要**逐篇记账**——哪几篇没退回来、因为什么。而记账要的是
 *  结构化的代号,`wireText` 给的是一句已经翻好的话,翻回去就没了。
 *
 *  实在认不出来的用 `unknown` 这个代号:**宁可让界面上出现一个
 *  看得见的 `unknown`,也不要记成空**——记成空等于把失败吞了,
 *  用户会看见"已撤销",而那一篇其实还带着原来的标签 */
export function toWireError(error: unknown): WireError {
  if (isWireError(error)) {
    return { code: error.code, args: error.args ?? {} };
  }
  if (error instanceof Error) return { code: "unknown", args: { detail: error.message } };
  return { code: "unknown", args: { detail: String(error) } };
}
