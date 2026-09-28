# Quire 架构

## 全貌

```
       路径 A:剪贴板(默认,零安装)         路径 B:扩展(可选)
┌─ 任意程序 ───────────────────────┐   ┌─ 当前网页 ─────────────────┐
│  你按 Ctrl+C                      │   │  点扩展图标                 │
│  · HTML Format —— 带结构+地址     │   │      │                      │
│  · Unicode Text —— 纯文本         │   │  content.js                │
└──────────────┬───────────────────┘   │  Defuddle 抽整页 + 元数据  │
               │                        │      │                      │
               │                        │  写进剪贴板(同左边)        │
               │                        └──────┼──────────────────────┘
               │                               │
               ▼  操作系统级共享资源,无授权环节、无监听端口
┌─ Tauri 桌面端 (Rust)─────────────────────────────────────────┐
│                                                                │
│  clipboard.rs ── OpenClipboard ── 读两种格式                  │
│      │  解析 CF_HTML:切字节偏移、取 quire-* 元数据            │
│      │  每 700ms 比对内容指纹(仅在「监控剪贴板」打开时)      │
│      ▼                                                         │
│    emit("clipboard-changed")                                   │
│         │                                                      │
│         ▼                                                      │
│  前端 (webview)                                                │
│      · 扩展来的:markdown 直接用,不再过 Defuddle               │
│      · 裸剪贴板:Defuddle 把 HTML 转成 Markdown                 │
│      · 弹提示条,用户点「保存」                                  │
│      ▼                                                         │
│  save_clip ── vault.rs ── 写 .md ── emit("clip-saved")        │
└────────────────────────────────────────────────────────────────┘
```

## 两条路径,一条链路

插件是**可选增强**,不是前提。关键在于两条路径最终都汇进同一个剪贴板读取口:

- **路径 A(默认)**:用户自己划选复制。零安装,拿得到片段和原文地址。
- **路径 B(可选)**:扩展对着整页跑一遍 Defuddle,把干净 HTML 和页面元数据写进剪贴板。Quire 那边一个字都不用改。

这样设计换来的是:**扩展不需要任何私有通道**。没有 localhost 端口,没有 native messaging,没有注册表,没有第二个目录要选。用户装扩展的动作从"配置一条 IPC 链路"降级成"加载一个文件夹"。

代价是路径 B 仍然要用户切窗口按 `Ctrl`+`V`(或者开着监控才能一键弹提示)。这是明知的取舍,换来的是零监听端口——对一个大半用户不装扩展的工具来说,这笔账很划算。

## 扩展与桌面端的约定

两边只靠一组 `<meta>` 键名通信,键名定义在 Rust 的 `EXTENSION_META` 和扩展的 `META`:

```
quire-version   协议版本,桌面端靠它判断"这份剪贴板是扩展写的"
quire-title     页面标题
quire-site      站点显示名
quire-author    作者
quire-excerpt   页面摘要
quire-published 发布时间
quire-image     封面图
```

外加 `source-url` 带原文地址——这是 CF_HTML 四种取址位置之一,复用现成的解析,不给地址另开一条路。

**统一 `quire-` 前缀不是洁癖。** 用户随时可能复制任意网页,那些页面自带 `author`、`description` 这类 meta;不加前缀,桌面端会把它们当成扩展填的元数据,用户看到作者栏凭空冒出个东西却不知道哪来的。测试 `网页自带的meta不会被误认成扩展元数据` 钉的就是这条。

**空值不传。** 扩展只写非空的键,桌面端也只收非空的值。两头都判一次空,少一个字段传了个 `content=""`,用户还得猜是页面没有还是坏了。

⚠️ **键名漂移不会有编译错误。** 两边是不同语言、不同进程,改了一边另一边照旧编译通过,只是运行时字段变空。所以桌面端侧有测试钉死键名,改的时候两边必须同步。

## 剪贴板捕获:CF_HTML 的四个坑

规范见 [CF_HTML 格式](https://learn.microsoft.com/en-us/windows/win32/dataxchg/html-clipboard-format)。

**偏移是字节,不是字符。** `StartHTML` / `EndHTML` / `StartFragment` / `EndFragment` 都是**字节**偏移。中文在 UTF-8 里占 3 字节,拿字符串索引去切,正文一含中文就错位,而且错得很安静——切出来的 HTML 看着还挺像样。`parse_cf_html` 因此全程在 `raw.as_bytes()` 上切片。测试 `正文含中文时偏移仍然正确` 锁的就是这条。

**CF_HTML 是 UTF-8,不是 UTF-16。** Windows 的剪贴板文本格式几乎清一色 UTF-16,`CF_HTML` 是那个少见的例外。照惯例去解 UTF-16 会得到满屏乱码。

**换行符有三种。** 规范说 header 行可能是 CRLF、LF,甚至是孤立的 CR。所以解析 header 是按"找到第一个不像 `Key:Value` 的行就停"来的,而不是按固定分隔符。

**原文地址有四个可能的位置。** 不同程序习惯不同,挨个兜住,少一个就可能整条回溯链断掉:

1. `SourceURL:` 头部(IE / MSHTML 系)
2. `<meta name="source-url" content="...">`
3. `<meta http-equiv="refresh" content="0; url=...">`
4. `<base href="...">`

只认 http/https,`javascript:` / `data:` 直接丢弃——它们拼进 frontmatter 是安全隐患。四个位置都没有就返回 `None`,**不编造**。前端此时不渲染「打开原文」,总比给个指向空白页的假入口强。

`arboard`(Tauri 剪贴板插件的底层实现)只支持文本和图片,没有 HTML 格式,所以这段是手写 Win32 调用。`CF_UNICODETEXT` 这个常量住在 `Win32::System::Ole` 命名空间下,看着别扭却是 Windows API 的既定事实。

## 信任边界

早期版本的本机 HTTP 服务带来一整类问题:只绑 `127.0.0.1` 并不等于安全,用户浏览器里开着的任何网页都能往那个端口发请求,得靠 Origin 校验挡(详见 git 历史)。现在服务整个删掉了,那类问题连同它的补丁一起消失。**扩展回来时也没有把它请回来**——扩展走剪贴板,不重蹈覆辙。

**新边界是:剪贴板内容一律当不可信输入。** 它来自任何程序——恶意网页完全可以构造一份精心设计的 HTML 让你复制。三道防线:

1. `markdown-it` 配 `html: false`,markdown 里的原始 HTML 不参与解析
2. `DOMPurify` 兜住解析器本身的疏漏(前一道挡不住 markdown-it 自己生成的 `<a href="javascript:...">`)
3. 图片加 `referrerPolicy="no-referrer"`,不把用户读了什么告诉图片服务器

装了扩展等于多信了一方。扩展只读当前页的 DOM、只写剪贴板、**不联网、不发任何请求**,权限只要一个 `clipboardWrite` 和 `activeTab`。它写进剪贴板的元数据照样走上面那三道防线——扩展能给的也只是建议,渲染前一律当外来内容。

**监控默认关闭。** 关着的时候 Quire 一个字节都不读剪贴板。开着时也只做两件事:比对内容指纹、弹提示——**不自动保存、不上传、不留历史**。默认开启的剪贴板监听会把密码、验证码、快递单号一起捕获,太吵,得由用户自己决定。

**监控只通知,不落盘。** 自动存等于替用户做决定;而且开着的时候什么东西都往里灌,用户会失去对"什么进了剪藏"的掌控。

**轮询而不是事件。** 700ms 一次,靠内容指纹去重,变了才 `emit`。Win32 的剪贴板变更通知需要注册窗口消息并在消息循环里泵,复杂度高一档,第一版没做。轮询的代价是提示最多晚半秒,以及一个始终醒着的后台线程——但它不读数据,关着的时候连 `OpenClipboard` 都不调。

## 数据格式

```markdown
---
id: "m8x2k9a4"
title: "本地优先的稍后读"
url: "https://example.com/post/1"
site: "example.com"
author: "张三"
clipped_at: "2026-09-28T14:30:00+08:00"
published_at: "2026-09-20T08:00:00+08:00"
excerpt: "为什么我们还在用云端笔记?"
cover: "https://example.com/cover.png"
tags: []
read: false
archived: false
---

正文…
```

### 几个刻意的选择

**Markdown 是唯一真源。** 没有数据库,列表直接读文件。索引(第二周的 FTS5)只是可重建的派生物,删掉不影响一个字节的数据。

**id 定长 16 字符。** 9 位毫秒时间戳(base36)+ 7 位随机盐。定长是**字典序等于时间序**的前提:变长的话 `m1zz…` 会排在 `m2aa…` 前面,时间序就废了。第二周上 FTS5 后,检索结果按 id 倒序就等于按时间倒序,不必再排 `clipped_at` 列;在文件管理器里按名称排序也天然是新到旧。

**时间一律 ISO 8601 带时区偏移。** 不存裸 `YYYY-MM-DD`——那东西被 `new Date()` 解析会按 UTC 算,凌晨剪的东西会落到前一天。

**文件名基于 URL 主机名,不采信 `siteName`。** Defuddle 的 `site` 字段在页面缺 `og:site_name` 时会退化成**作者名**,拿它命名会得到 `2026-09-28-xxx-张三.md`。反正主机名能从 URL 算出来,没理由去信一个会漂的字段。`site` 字段仍保留,仅用于显示。回归测试在 `vault.rs`。

**`read` / `archived` 放 frontmatter 而不放数据库。** 第三周的未读队列直接读文件。索引丢了,数据还在。

**手写 frontmatter 解析器,不引 YAML crate。** 支持的只是 `"..."` 字符串、裸串、`true`/`false`、整数、`[a, b]` 列表、裸键——不认识的写法当纯字符串,不报错也不丢数据。理由:frontmatter 是我们自己写死的格式,解析器本身就是一份长期契约;用户会在任何编辑器里手改这些文件,十年后还得读得懂。`gray_matter` 停在 2021 年的 0.0.1,不值得为它引一个行为不受控的依赖。

**未知字段原样保留。** 用户手写的自定义键进 `extra` 兜着,不会因为 Quire 升级就消失。数据是用户的。

**落盘走临时文件 + rename。** Windows 的 `rename` 不覆盖已存在文件,万一盐值碰撞撞了名(概率极低),删掉目标重试,好过留下一个写到一半的剪藏。

**读不出来的文件要报出来。** `scan()` 把解析失败的 `.md` 放进 `unreadable` 列表返回给前端,而不是静默跳过。vault 里的文件用户可能正手动编辑,默默跳过等于让人以为剪藏丢了。

## 抽取:两个必须记住的坑

**`markdown` 与 `separateMarkdown` 互斥。** 只开 `markdown: true` 时,`content` 本身就是 Markdown,`contentMarkdown` 保持 `undefined`;只开 `separateMarkdown: true` 时,`content` 保持 HTML,Markdown 去 `contentMarkdown` 取。同时开两个,结果是读一个永远是 `undefined` 的字段,于是**静默存进一堆空剪藏**。

**两条路径故意用不同的开关,不是笔误:**

| | 谁转换 | 开关 | `content` | `contentMarkdown` |
| --- | --- | --- | --- | --- |
| 路径 A(裸剪贴板) | 桌面端 `clipToMarkdown` | `markdown: true` | Markdown | `undefined` |
| 路径 B(扩展) | 扩展 `content.ts` | `separateMarkdown: true` | 干净 HTML | Markdown |

路径 B 要 HTML,是因为剪贴板得带两种格式:`text/html` 给别的应用用,`text/plain` 装 Markdown 给 Quire。路径 A 只需要 Markdown。桌面端收到 `quire-version` 就知道是哪条,直接用 `text`,**不再过第二遍 Defuddle**——干净内容被再加工一次只会多一处出错的机会。

**`site` 会退化成作者名。** 见上文"文件名基于 URL 主机名"。扩展里也防了这手:`pickSite()` 发现 `site === author` 就退回域名,免得 frontmatter 里写出"站点:张三"。

> ⚠️ 前端这两条分支**没有自动化测试**。补测试要给前端引 DOM 测试环境(vitest + jsdom 之类),这是个新增依赖,尚未决定。桌面端那一侧倒是有真实剪贴板往返测试兜着。

## 测试分层

| 层 | 覆盖什么 | 怎么测 |
| --- | --- | --- |
| `clipboard.rs` | CF_HTML 解析、URL 四个来源、字节偏移、换行符、非法协议、`quire-*` 元数据 | 20 条纯函数单测 |
| `clipboard.rs` | **真实的 Win32 读写链路** | 2 条往返测试:往真剪贴板写数据再读回来比对(一条裸剪贴板、一条扩展载荷) |
| `vault.rs` | 落盘、扫描、frontmatter 往返、路径穿越防护 | 11 条单测 |
| `slug.rs` / `ids.rs` | 文件名白名单、id 时间序 | 单测 |
| 前端 | Defuddle 调用、两条路径分流、降级链、DOMPurify | ❌ 无自动化测试(见上) |
| 扩展 | 打包产物自包含、manifest 引用完整 | 构建时校验 |

那两条往返测试是整个 `platform` 模块唯一的覆盖。纯函数测试全绿也证明不了 `RegisterClipboardFormatW` 的格式名没拼错、句柄类型没搞混——这类错误在真实调用里才会暴露。两点值得记:

- 用 `Drop` 守卫保证即使断言 panic 也会还原用户原本的剪贴板。
- **必须串行。** 剪贴板是全机器唯一的资源,cargo 默认并行跑测试,不加锁两条测试会互相盖掉对方的数据,表现为"单跑绿、全跑红",而且红的行号每次都不一样。用 `CLIPBOARD_LOCK` 排队。

## 中文搜索(第二周)

不用 trigram。FTS5 的 trigram 分词器有硬下限:**少于 3 个 unicode 字符的查询匹配不到任何行**,而中文里「苹果」「淘宝」「编程」全是 2 字。

方案是 `libsimple`(jieba 分词 + 拼音),一套覆盖中英混排。索引是可重建的派生物,删掉不影响 `.md` 文件。

## 构建环境备注

Windows 上构建 Tauri **不需要 Windows SDK**。Rust 1.92+ 的 MSVC target 会在链接时自动生成导入库(已实测 `#[link(name = "user32")]` 在无 SDK 环境下链接通过)。

但 Tauri 仍需要 `icons/icon.ico`——它在 Windows 上要生成资源信息文件(版本号、图标)。这个跟 SDK 是两码事。
