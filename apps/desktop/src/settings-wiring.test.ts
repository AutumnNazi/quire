import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 设置面板接上了没有。
 *
 * 要挡的回归都很安静:面板画出来了、按钮也在,可它读的是一份跟真正在用的
 * 那份不同步的状态,或者迁移那一步悄悄把剪藏留在旧目录里——运行时不报任何错,
 * 用户是过了两个月回来找东西才发现的。所以这里盯的全是**数据流**:
 * 面板显示的路径、开关拨到后端的状态、迁移之后有没有刷新,各自那条线通没通。
 */
describe("设置面板接上了没有", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");

  /** 取一整个函数体。理由和 `stats-wiring.test.ts` 里那个一样:
   *  直接找第一个 `{` 会撞上类型注解,只找 `)` 会撞上参数里的括号 */
  const body = (anchor: string): string => {
    const at = mainSrc.indexOf(anchor);
    if (at < 0) return "";
    let depth = 0;
    let paramsClosed = -1;
    for (let i = at; i < mainSrc.length; i += 1) {
      if (mainSrc[i] === "(") depth += 1;
      else if (mainSrc[i] === ")") {
        depth -= 1;
        if (depth === 0) {
          paramsClosed = i;
          break;
        }
      }
    }
    if (paramsClosed < 0) return "";
    const open = mainSrc.indexOf("{", paramsClosed);
    depth = 0;
    for (let i = open; i < mainSrc.length; i += 1) {
      if (mainSrc[i] === "{") depth += 1;
      else if (mainSrc[i] === "}") {
        depth -= 1;
        if (depth === 0) return mainSrc.slice(at, i + 1);
      }
    }
    return mainSrc.slice(at);
  };

  /** 工具栏模板本身。`root.innerHTML` 到收尾反引号那一段 */
  const toolbar = (): string => {
    const start = mainSrc.indexOf("root.innerHTML = `");
    if (start < 0) return "";
    const end = mainSrc.indexOf("\n`;", start);
    return end < 0 ? "" : mainSrc.slice(start, end);
  };

  it("工具栏上有入口,而且按钮接上了", () => {
    // 没有按钮的话这个面板对用户来说就是不存在的东西
    expect(mainSrc, "工具栏上得有设置按钮").toContain('id="btn-settings"');
    expect(
      mainSrc,
      "按钮得接上打开/关闭",
    ).toMatch(/settingsBtnEl\.addEventListener\("click",[\s\S]{0,160}openSettings\(\)/);
  });

  it("换目录、切语言、监控开关都不再占工具栏", () => {
    // 这三样都是低频动作。**摆在工具栏上等于天天占位给一年用一次的功能**,
    // 而工具栏每多一个按钮,真正每天用的那几个就被挤窄一格
    const tpl = toolbar();
    expect(tpl.length, "没找到工具栏模板").toBeGreaterThan(0);
    expect(tpl, "换目录搬进设置面板了").not.toContain('id="btn-pick"');
    expect(tpl, "切语言搬进设置面板了").not.toContain('id="btn-lang"');
    expect(tpl, "监控开关搬进设置面板了").not.toContain('id="chk-watch"');
    // 搬走了就得真的在面板里,不然就是"功能没了"
    const panel = body("function renderSettingsPanel(box: HTMLElement)");
    expect(panel.length, "没找到 renderSettingsPanel").toBeGreaterThan(0);
    expect(panel, "面板里得有换目录").toContain("changeVaultWithMigration()");
    expect(panel, "面板里得有监控开关").toContain('type = "checkbox"');
    expect(panel, "面板里得有语言切换").toContain("setLocale(otherLocale())");
  });

  it("Esc 关得掉", () => {
    // 一个关不掉的面板比没有面板更糟:它挡着正文,用户只能重启软件
    const at = mainSrc.indexOf('e.key === "Escape" && settingsPanelEl');
    expect(at, "Esc 那条得认这个面板").toBeGreaterThan(-1);
    expect(mainSrc.slice(at, at + 160), "认得就得关").toContain("closeSettings()");
  });

  it("面板上显示的路径就是渲染详情时用的那一份", () => {
    // 两条各自缓存的路径必然漂移:用户在设置里看到 D:\Quire,打开某篇却
    // 图片全裂。**必须是同一个变量**,不是"两份值碰巧一样"
    const panel = body("function renderSettingsPanel(box: HTMLElement)");
    expect(panel, "路径得取自 vaultRoot").toContain("vaultRoot");
    const load = body("async function loadVaultInfo()");
    expect(load, "vaultRoot 由 loadVaultInfo 写入").toMatch(/vaultRoot\s*=\s*info\.path/);
  });

  it("开关拨到后端去,不是只改界面上的勾", () => {
    // **只改界面上的勾是最安静的一种坏**:面板看着是开着的,后端没开,
    // 用户复制一篇文章等半天没反应,最后认定软件坏了
    const fn = body("async function setWatchToggle(next: boolean)");
    expect(fn.length, "没找到 setWatchToggle").toBeGreaterThan(0);
    expect(fn, "得真调后端").toContain("api.setClipboardWatch(next)");
    // 界面状态得跟着后端回来的那份走,不是自己乐观地写上去
    expect(fn, "以后端回的为准").toMatch(/watchOn\s*=\s*info\.watching/);
    expect(fn, "失败要把勾拨回去").toMatch(/watchOn\s*=\s*!next/);
  });

  it("面板开关和首次询问走同一条路", () => {
    // 两条路各调一次后端,就有一份能开不能关。**共用一个函数**是唯一
    // 能保证两边行为一样的办法
    const panel = body("function renderSettingsPanel(box: HTMLElement)");
    expect(panel, "面板开关得调那个共用函数").toContain("setWatchToggle(");
    const ask = body("async function askWatchOnce(enable: boolean)");
    expect(ask, "首次询问得调同一个后端命令").toContain("api.setClipboardWatch(enable)");
  });

  it("选目录时点了取消就什么都不发生", () => {
    // 返回 null 是用户在文件夹对话框里点了取消。**这时候不许切库**——
    // 切了就等于用户想取消结果剪藏搬走了
    const fn = body("async function changeVaultWithMigration()");
    expect(fn.length, "没找到 changeVaultWithMigration").toBeGreaterThan(0);
    expect(fn, "得判 null").toMatch(/if\s*[(]!target[)]\s*return/);
    expect(fn, "路径是选目录那个命令拿的").toContain("api.chooseVaultTarget()");
    // 判完之后不许再改写 target。`if (!target) return; target = vaultRoot;`
    // 看着像在补默认值,实际是把"取消"变成了"把库搬回它原来的位置"。
    // **只查判完之后那一段**——从选目录命令那里赋的值是本来就该有的
    const afterGuard = fn.slice(fn.search(/if\s*[(]!target[)]\s*return/));
    expect(afterGuard, "判完 null 之后不许再改写 target").not.toMatch(/\btarget\s*=[^=]/);
  });

  it("搬过去和从空目录开始是两个分开的决定,不是一个 yes/no", () => {
    // 合并成一个「确定吗」的话,默认的那一边就是丢数据的那一边。
    // 两个分支必须**各自把 copyExisting 传对**
    const fn = body("async function changeVaultWithMigration()");
    expect(fn, "搬过去的分支得传 true").toContain("finishMigration(target, true)");
    expect(fn, "空目录的分支得传 false").toContain("finishMigration(target, false)");
    expect(fn, "主按钮得是先不换").toContain('t("settings.migrate.cancel"), primary: true');
  });

  it("搬完得刷新列表,不然用户看着的还是旧库", () => {
    // 迁移只改了后端的库路径。**不重读的话界面上还是上一篇的正文**,
    // 而用户以为迁移没生效,又去搬一次
    const fn = body("async function finishMigration(target: string, copyExisting: boolean)");
    expect(fn.length, "没找到 finishMigration").toBeGreaterThan(0);
    expect(fn, "得真调后端迁移").toContain("api.migrateVault(target, copyExisting)");
    expect(fn, "得重读库路径").toContain("loadVaultInfo()");
    expect(fn, "得重画列表").toContain("refreshList()");
    // 面板上那个路径得跟着变,不然用户以为没搬成
    expect(fn, "面板得重画").toContain("syncSettingsPanel()");
  });

  it("搬了多少篇是事件带回来的,搬完还会说一声", () => {
    // `migrateVault` 只回库信息,篇数得靠 `vault-migrated` 事件。**没接这个
    // 事件的话用户只看见"切了",不知道搬没搬成功**——迁到一半失败正好是这个
    // 样子,界面上一片正常
    const at = mainSrc.indexOf('listen<VaultMigrated>("vault-migrated"');
    expect(at, "得订上 vault-migrated").toBeGreaterThan(-1);
    const listener = mainSrc.slice(at, at + 900);
    expect(listener, "得从 payload 里取篇数").toMatch(/e\.payload/);
    expect(listener, "得把篇数写进提示里").toContain('t("settings.migrate.done"');
    expect(listener, "有跳过的也得说出来").toContain('t("settings.migrate.doneSkipped"');
    // **篇数和图片数分开取。** 混成一个数字的话,两篇剪藏加二十几张图
    // 会报成「搬过去了 26 篇」——用户以为库里凭空多出一堆东西
    expect(listener, "得分别取 clips 和 assets").toMatch(
      /const \{ clips, assets, skipped \} = e\.payload/,
    );
    expect(listener, "提示里篇数用的是 clips").toMatch(/n: String\(clips\)/);
    expect(listener, "图片数单独带进文案").toMatch(/assets: String\(assets\)/);

    // 搬过去的那条提示只能有一处。**两处都报就是两条互相顶掉**,
    // 用户看见的是后到的那条,篇数没了
    const finish = body("async function finishMigration(target: string, copyExisting: boolean)");
    expect(finish, "搬过去的提示归事件管").not.toContain('t("settings.migrate.done"');
    expect(finish, "空目录那条还是得说一声").toContain('if (!copyExisting) showToast(t("settings.migrate.switched"))');
  });

  it("切语言后面板里的字也得跟着换", () => {
    // 面板是动态搭的,`applyI18n` 扫不到它。不重画的话用户点了语言,
    // 工具栏全变了,打开设置还是上一种语言——看着像一半的翻译坏了
    const fn = body("function applyLocale()");
    expect(fn, "得重画设置面板").toContain("syncSettingsPanel()");
    const sync = body("function syncSettingsPanel()");
    expect(sync, "面板没开着就别动").toContain("if (!settingsPanelEl) return;");
    expect(sync, "重画走的是同一个画法").toContain("renderSettingsPanel(settingsPanelEl)");
  });
});
