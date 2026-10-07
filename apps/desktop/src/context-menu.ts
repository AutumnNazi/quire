/**
 * 右键菜单。
 *
 * 单独成个模块的理由跟 `clip-item.ts` 一样:它是纯构建 + 自管生命周期,
 * 不碰任何业务状态——谁都可以借去弹一个菜单,不用把弹层逻辑抄进自己
 * 的代码里。
 *
 * **单例。** 屏幕上同时只能有一个右键菜单:再开一个时旧的先关。做多个
 * 实例的话,Esc 该关哪一个、点外面该关哪一个,马上变成谁也说不清的事。
 */

export interface MenuItem {
  /** 菜单上的文字。由调用方给:菜单不管文案从哪本词典来。 */
  label: string;
  /** 悬停说明,可不带。 */
  title?: string;
  /** 危险动作(删除这类):红字,并且调用方应该把它排在最后。 */
  danger?: boolean;
  onClick: () => void;
}

let menuEl: HTMLElement | null = null;

export function closeContextMenu(): void {
  if (!menuEl) return;
  menuEl.remove();
  menuEl = null;
  // 监听跟着菜单一起摘。挂在 open 这边、忘了在 close 这边摘的话,
  // 每开一次就多一份全局监听,关第 N 次要按 N 下 Esc
  document.removeEventListener("mousedown", onOutside, true);
  document.removeEventListener("keydown", onKey, true);
  document.removeEventListener("scroll", onScroll, true);
}

function onOutside(e: MouseEvent): void {
  if (menuEl && !menuEl.contains(e.target as Node)) closeContextMenu();
}

function onKey(e: KeyboardEvent): void {
  if (e.key === "Escape") {
    e.preventDefault();
    closeContextMenu();
  }
}

/** 菜单挂在 body 上不随列表滚。列表一动,菜单指的位置就是空的——关掉 */
function onScroll(): void {
  closeContextMenu();
}

/** 在 (x, y) 处弹菜单。**先量再摆**:菜单不量就摆,靠屏幕右缘/下缘的
 *  那一份会伸出屏幕外,看得见的那半截还是最不重要的几项 */
export function openContextMenuAt(x: number, y: number, items: MenuItem[]): void {
  closeContextMenu();
  // 空菜单不开。挂一个空壳在屏幕上,用户只会觉得"右键了但什么都没发生"
  if (items.length === 0) return;

  const menu = document.createElement("div");
  menu.className = "context-menu";
  menu.setAttribute("role", "menu");
  for (const item of items) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = item.danger ? "context-menu-item danger" : "context-menu-item";
    btn.setAttribute("role", "menuitem");
    btn.textContent = item.label;
    if (item.title) btn.title = item.title;
    btn.addEventListener("click", () => {
      // 先关再执行:执行里可能又弹别的面板,让菜单压在面板上面就错了
      closeContextMenu();
      item.onClick();
    });
    menu.append(btn);
  }
  document.body.append(menu);

  const rect = menu.getBoundingClientRect();
  const px = Math.min(x, window.innerWidth - rect.width - 8);
  const py = Math.min(y, window.innerHeight - rect.height - 8);
  menu.style.left = `${Math.max(4, px)}px`;
  menu.style.top = `${Math.max(4, py)}px`;

  menuEl = menu;
  // capture:菜单外的元素自己也可能监听 mousedown,先于它们关菜单,
  // 不然点菜单外面会把"关闭菜单"和"那边的动作"各执行一遍
  document.addEventListener("mousedown", onOutside, true);
  document.addEventListener("keydown", onKey, true);
  document.addEventListener("scroll", onScroll, true);
}
