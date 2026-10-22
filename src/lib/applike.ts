// Links in the app move between screens; they never open browser windows or get dragged out.

/** Should this click on a link be kept inside the app (no new window or tab)? */
export function keepInApp(e: { button: number; ctrlKey: boolean; shiftKey: boolean; metaKey: boolean }): boolean {
  return e.button === 1 || e.ctrlKey || e.shiftKey || e.metaKey;
}

export function installAppLike(doc: Document): void {
  const link = (t: EventTarget | null) => (t instanceof Element ? t.closest("a[href]") : null);
  // Middle click, Ctrl+click and Shift+click would open a new browser window.
  doc.addEventListener("auxclick", (e) => {
    if (link(e.target)) e.preventDefault();
  });
  doc.addEventListener(
    "click",
    (e) => {
      const a = link(e.target) as HTMLAnchorElement | null;
      if (!a || !keepInApp(e)) return;
      e.preventDefault();
      // Go there in this window, as a plain click would.
      const href = a.getAttribute("href") ?? "";
      if (href.startsWith("#")) location.hash = href;
    },
    true,
  );
  // Links and pictures can't be dragged out of the window.
  doc.addEventListener("dragstart", (e) => {
    const t = e.target;
    if (t instanceof Element && (t.closest("a[href]") || t.tagName === "IMG")) e.preventDefault();
  });
}
