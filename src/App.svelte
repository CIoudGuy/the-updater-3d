<script lang="ts">
  import { onMount } from "svelte";
  import TopBar from "./components/TopBar.svelte";
  import PopoverLayer from "./components/PopoverLayer.svelte";
  import DialogHost from "./components/DialogHost.svelte";
  import Toasts from "./components/Toasts.svelte";
  import Home from "./screens/Home.svelte";
  import Installed from "./screens/Installed.svelte";
  import AppDetail from "./screens/AppDetail.svelte";
  import GetApps from "./screens/GetApps.svelte";
  import GetDetail from "./screens/GetDetail.svelte";
  import Queue from "./screens/Queue.svelte";
  import Activity from "./screens/Activity.svelte";
  import Services from "./screens/Services.svelte";
  import Settings from "./screens/Settings.svelte";
  import Skeleton from "./components/Skeleton.svelte";
  import { router, type Route } from "./lib/router.svelte";
  import { store } from "./lib/store.svelte";
  import { ui } from "./lib/ui.svelte";
  import { send, windowApi } from "./lib/api";
  import { reduced } from "./lib/motion";
  import { checkNow, isPaused, togglePauseAll, updateAll, updateOne } from "./lib/actions";
  import { tick } from "svelte";
  import { applyZoom, clampZoom, cssScale, currentZoom, stepZoom, zoomPercent } from "./lib/zoom";

  // ----- page transitions: old page fades out fast, new page fades in and rises -----
  let shownKey = router.key;
  let shown = $state<Route>(router.route);
  let phase = $state<"in" | "out">("in");
  let page = $state<HTMLElement>();
  let timer: ReturnType<typeof setTimeout> | undefined;

  // The scroll position of the page on screen is remembered per history entry (see router.svelte.ts).
  router.scrollSource = () => (phase === "in" && page ? page.scrollTop : null);
  router.canStep = () => !ui.dialog;
  let handledNav = router.navId;

  /** Puts the page back where it was. Content can still be loading, so it keeps trying for a moment, and stops when the user scrolls. */
  function restoreScroll(el: HTMLElement, top: number) {
    const until = performance.now() + 2000;
    let timer: ReturnType<typeof setInterval>;
    const finish = () => {
      clearInterval(timer);
      el.removeEventListener("wheel", finish);
      el.removeEventListener("pointerdown", finish);
      window.removeEventListener("keydown", finish);
    };
    el.addEventListener("wheel", finish, { passive: true });
    el.addEventListener("pointerdown", finish);
    window.addEventListener("keydown", finish);
    const step = () => {
      el.scrollTop = top;
      if (Math.abs(el.scrollTop - top) <= 2 || performance.now() > until) finish();
    };
    timer = setInterval(step, 30);
    step();
  }
  function settle(top: number | null) {
    page?.scrollTo({ top: 0 });
    if (top) tick().then(() => page && restoreScroll(page, top));
  }

  $effect(() => {
    const r = router.route;
    const key = router.key;
    const nav = router.navId;
    const top = router.pendingScroll;
    const fresh = nav !== handledNav;
    handledNav = nav;
    if (key === shownKey) {
      shown = r;
      // back or forward between two entries of the same page
      if (fresh && top != null) tick().then(() => page && restoreScroll(page, top));
      return;
    }
    phase = "out";
    clearTimeout(timer);
    timer = setTimeout(
      () => {
        shownKey = key;
        shown = r;
        phase = "in";
        settle(top);
      },
      reduced() ? 0 : 90,
    );
  });

  onMount(() => {
    let off = () => {};
    store.init((route) => router.go(route.replace(/^#?\/?/, ""))).then((u) => (off = u));
    // "Fewer admin prompts" on but not set up yet: its one admin prompt comes now, not mid-update.
    send("window_ready", {});
    windowApi.isMaximized().then((m) => (ui.maximized = m));
    const onReject = (e: PromiseRejectionEvent) => {
      e.preventDefault();
      const r = e.reason;
      store.toast("err", typeof r === "string" ? r : (r?.message ?? "Something went wrong"));
    };
    window.addEventListener("unhandledrejection", onReject);
    window.addEventListener("resize", measure);
    measure();

    // Ctrl + wheel zooms the app; the browser's own zoom must not run on top of it.
    let wheelAcc = 0;
    let wheelLock = 0;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      const now = performance.now();
      if (now < wheelLock) return;
      wheelAcc += e.deltaY;
      if (Math.abs(wheelAcc) < 8) return;
      const dir = wheelAcc < 0 ? 1 : -1;
      wheelAcc = 0;
      wheelLock = now + 70;
      setZoom(stepZoom(dir));
    };
    window.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      window.removeEventListener("wheel", onWheel);
      off();
      window.removeEventListener("resize", measure);
      window.removeEventListener("unhandledrejection", onReject);
    };
  });

  // ----- zoom -----
  const measure = () => (ui.width = window.innerWidth / cssScale());
  function setZoom(z: number) {
    z = clampZoom(z);
    const changed = z !== currentZoom();
    void applyZoom(z).then(measure);
    store.toast("info", `Zoom ${zoomPercent(z)}`, undefined, { key: "zoom", ms: 1400 });
    if (changed) store.patchSettings({ uiZoom: z });
  }
  // the saved zoom is the truth once the settings have arrived
  $effect(() => {
    if (!store.ready) return;
    const want = clampZoom(store.settings.uiZoom || 1);
    if (want !== currentZoom()) void applyZoom(want).then(measure);
  });

  // ----- keyboard -----
  const ROUTES = ["home", "installed", "get", "queue", "activity", "services"];
  function focusSearch() {
    const name = router.route.name;
    if (name === "installed" || name === "get") {
      ui.searchEl?.focus();
      ui.searchEl?.select();
    } else {
      router.go("installed");
      tick().then(() => setTimeout(() => ui.searchEl?.focus(), 140));
    }
  }
  const blockedOpen = () => store.blockedJobs.some((j) => !ui.dismissedBlocked.includes(j.id));

  function onkeydown(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    const typing = !!t && (t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || t.isContentEditable);

    if (e.key === "Escape") {
      if (ui.escape()) return e.preventDefault();
      if (!ui.dialog && blockedOpen()) {
        const b = store.blockedJobs.find((j) => !ui.dismissedBlocked.includes(j.id));
        if (b) ui.dismissedBlocked = [...ui.dismissedBlocked, b.id];
        return e.preventDefault();
      }
      if (typing) {
        (t as HTMLInputElement).blur();
        return;
      }
      if (store.toasts.length) store.toasts = [];
      return;
    }
    if ((e.ctrlKey || e.metaKey) && !e.altKey) {
      const code = e.code;
      if (e.key === "=" || e.key === "+" || code === "NumpadAdd") {
        e.preventDefault();
        return setZoom(stepZoom(1));
      }
      if (e.key === "-" || e.key === "_" || code === "NumpadSubtract") {
        e.preventDefault();
        return setZoom(stepZoom(-1));
      }
      if (e.key === "0" || code === "Numpad0") {
        e.preventDefault();
        return setZoom(1);
      }
    }
    if (ui.dialog || blockedOpen()) return;

    if (e.key === "F5") {
      e.preventDefault();
      checkNow();
      return;
    }
    if (e.altKey && !e.ctrlKey && !e.metaKey && e.key === "ArrowLeft") {
      e.preventDefault();
      router.back();
      return;
    }
    if (e.altKey && !e.ctrlKey && !e.metaKey && e.key === "ArrowRight") {
      e.preventDefault();
      router.forward();
      return;
    }
    if (e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey) {
      const k = e.key;
      if (k >= "1" && k <= "6") {
        e.preventDefault();
        router.go(ROUTES[Number(k) - 1]);
      } else if (k === ",") {
        e.preventDefault();
        router.go("settings");
      } else if (k.toLowerCase() === "u") {
        e.preventDefault();
        updateAll();
      } else if (k.toLowerCase() === "p") {
        e.preventDefault();
        togglePauseAll();
      } else if (k.toLowerCase() === "f") {
        e.preventDefault();
        focusSearch();
      } else if (k.toLowerCase() === "k") {
        e.preventDefault(); // no command palette
      }
      return;
    }
    if (typing || ui.popover || e.ctrlKey || e.altKey || e.metaKey) return;

    const name = router.route.name;
    const k = e.key.toLowerCase();
    if (k === "u" && (name === "installed" || name === "detail" || name === "home")) {
      const id = name === "detail" ? router.route.param : ui.focusedApp;
      if (id) {
        e.preventDefault();
        updateOne(id);
      }
    } else if (name === "installed" && k === "g") {
      ui.setView("grid");
    } else if (name === "installed" && k === "l") {
      ui.setView("list");
    } else if (k === " " && name === "queue") {
      /* space is handled by the focused button */
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="app">
  <TopBar />
  <main class="page {phase}" bind:this={page}>
    {#if !store.ready}
      <Skeleton name={shown.name} />
    {:else}
      {#if shown.name === "home"}
        <Home />
      {:else if shown.name === "installed"}
        <Installed />
      {:else if shown.name === "detail" && shown.param}
        {#key shown.param}<AppDetail id={shown.param} />{/key}
      {:else if shown.name === "get"}
        <GetApps />
      {:else if shown.name === "getdetail" && shown.param}
        {#key shown.param}<GetDetail id={shown.param} />{/key}
      {:else if shown.name === "queue"}
        <Queue />
      {:else if shown.name === "activity"}
        <Activity />
      {:else if shown.name === "services"}
        <Services />
      {:else if shown.name === "settings"}
        <Settings section={shown.param} />
      {/if}
    {/if}
  </main>
  <PopoverLayer />
  <DialogHost />
  <Toasts />
</div>

<style>
  .app {
    /* CSS zoom (browser preview only) scales vh as well, so divide it back out */
    height: calc(100vh / var(--zoom, 1));
    display: flex;
    flex-direction: column;
    background: var(--bg);
    overflow: hidden;
  }
  .page {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow-x: hidden;
    overflow-y: auto;
  }
  .page.in {
    animation: page-in 0.12s var(--ease) both;
  }
  .page.out {
    animation: page-out 0.06s linear forwards;
    pointer-events: none;
  }
</style>
