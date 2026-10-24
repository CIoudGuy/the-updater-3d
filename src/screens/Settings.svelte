<script lang="ts">
  import { onMount, tick } from "svelte";
  import SRow from "../components/SRow.svelte";
  import Toggle from "../components/Toggle.svelte";
  import Segmented from "../components/Segmented.svelte";
  import Select from "../components/Select.svelte";
  import Btn from "../components/Btn.svelte";
  import Mark from "../components/Mark.svelte";
  import Paint from "../components/Paint.svelte";
  import Icon from "../components/Icon.svelte";
  import Dab from "../components/Dab.svelte";
  import { I } from "../lib/icons";
  import { call, on, send } from "../lib/api";
  import { store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { router, SETTINGS_SECTIONS, type SettingsSection } from "../lib/router.svelte";
  import { bytes, clock, driveOf } from "../lib/format";
  import { setIgnored, updateSelf } from "../lib/actions";
  import { MARKER_BASE, pageIn, pageOut, place, rise } from "../lib/motion";
  import type { AccountInfo, AutoMode, Platform } from "../lib/types";

  let { section = null }: { section: string | null } = $props();

  const s = $derived(store.settings);

  interface NavItem {
    id: SettingsSection;
    label: string;
    blurb: string;
  }
  const NAV: NavItem[] = [
    { id: "general", label: "General", blurb: "When to check, what to do with updates, and notifications." },
    { id: "apps", label: "Apps", blurb: "Which apps The Updater 3D looks after, and their options." },
    { id: "games", label: "Games", blurb: "Sign in to Steam, Epic Games and GOG to see every game you own, installed or not." },
    { id: "downloads", label: "Downloads", blurb: "How many apps download at once, and how fast." },
    { id: "startup", label: "Startup", blurb: "Keep checking for updates when the window is closed." },
    { id: "cleanup", label: "Cleanup", blurb: "Free up space and tidy what removed apps left behind." },
    { id: "adobe", label: "Adobe", blurb: "Adobe apps are installed through Creative Cloud." },
    { id: "about", label: "About", blurb: "This version of The Updater 3D." },
  ];

  // Pages that were renamed or merged, so old links still land in the right place.
  const OLD: Record<string, SettingsSection> = { sources: "apps", updates: "general", notifications: "general", background: "startup", storage: "cleanup", account: "adobe" };
  const active = $derived.by((): SettingsSection => {
    const want = section ? (OLD[section] ?? section) : null;
    if (want === "adobe" && !store.adobeOn) return "apps";
    return want && (SETTINGS_SECTIONS as readonly string[]).includes(want) ? (want as SettingsSection) : "general";
  });
  // The Adobe page is all Creative Cloud: only shown while Adobe apps are on (Apps keeps the switch).
  const nav = $derived(NAV.filter((n) => n.id !== "adobe" || store.adobeOn));
  const idx = $derived(nav.findIndex((n) => n.id === active));
  const cur = $derived(nav[idx] ?? nav[0]);

  let dir = $state(1);
  let prevIdx = -1;
  $effect.pre(() => {
    const i = idx;
    if (prevIdx >= 0 && i !== prevIdx) dir = i > prevIdx ? 1 : -1;
    prevIdx = i;
  });

  let root = $state<HTMLElement>();
  let navEl = $state<HTMLElement>();
  let marker = $state<HTMLElement>();
  let anim = $state(false);
  const navBtns: Record<string, HTMLElement | undefined> = {};

  function layout() {
    if (marker) place(marker, navBtns[active]);
  }
  $effect(() => {
    active;
    tick().then(layout);
  });
  $effect(() => {
    // a new page starts at the top
    active;
    root?.closest(".page")?.scrollTo({ top: 0 });
  });
  onMount(() => {
    const ro = new ResizeObserver(layout);
    if (navEl) ro.observe(navEl);
    const id = requestAnimationFrame(() => {
      layout();
      anim = true;
    });
    return () => {
      ro.disconnect();
      cancelAnimationFrame(id);
    };
  });

  function pick(id: string) {
    if (id !== active) router.replace("settings/" + id);
  }
  /** Up and Down move through the pages while the nav has focus. */
  function navKey(e: KeyboardEvent) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const n = nav[Math.max(0, Math.min(nav.length - 1, idx + (e.key === "ArrowDown" ? 1 : -1)))];
    pick(n.id);
    navBtns[n.id]?.focus();
  }

  // ----- option lists -----
  const intervalOpts = [
    { value: "1", label: "Every hour" },
    { value: "2", label: "Every 2 hours" },
    { value: "4", label: "Every 4 hours" },
    { value: "8", label: "Every 8 hours" },
    { value: "12", label: "Every 12 hours" },
    { value: "24", label: "Once a day" },
    { value: "0", label: "Never" },
  ];
  const modeOpts: { value: AutoMode; label: string }[] = [
    { value: "off", label: "Off" },
    { value: "notify", label: "Notify only" },
    { value: "install", label: "Install automatically" },
  ];
  /** An app's own rule can also turn its updates off entirely. */
  const ruleOpts: { value: AutoMode | "ignore"; label: string }[] = [...modeOpts, { value: "ignore", label: "Don't update" }];
  const hours = Array.from({ length: 24 }, (_, i) => ({ value: `${String(i).padStart(2, "0")}:00`, label: `${String(i).padStart(2, "0")}:00` }));
  const timeOpts = (cur: string) => (hours.some((h) => h.value === cur) ? hours : [...hours, { value: cur, label: cur }].sort((a, b) => a.value.localeCompare(b.value)));
  const parOpts = [1, 2, 3, 4].map((n) => ({ value: String(n), label: String(n) }));
  const bwOpts = $derived.by(() => {
    const cur = Math.round((s.bandwidthLimit || 40e6) / 1e6);
    const base = [5, 10, 20, 40, 80, 150];
    if (!base.includes(cur)) base.push(cur);
    return base.sort((a, b) => a - b).map((n) => ({ value: String(n), label: `${n} MB/s` }));
  });
  const langOpts = [
    { value: "en_US", label: "English (US)" },
    { value: "en_GB", label: "English (UK)" },
    { value: "de_DE", label: "German" },
    { value: "fr_FR", label: "French" },
    { value: "es_ES", label: "Spanish" },
    { value: "ja_JP", label: "Japanese" },
  ];

  const ignoredIds = $derived(s.ignored ?? []);
  const exceptionIds = $derived([...new Set([...Object.keys(s.exceptions), ...ignoredIds])]);
  /** The rule shown for an app: its exception, or "Don't update" */
  const ruleOf = (id: string) => (ignoredIds.includes(id) ? "ignore" : s.exceptions[id]);
  function setException(id: string, mode: AutoMode | "ignore" | null) {
    if (mode === "ignore") return setIgnored(id, true);
    const next = { ...s.exceptions };
    if (mode) next[id] = mode;
    else delete next[id];
    store.patchSettings({ exceptions: next, ignored: ignoredIds.filter((x) => x !== id) });
  }
  /** Settings > About: only this app's own update, not the apps' */
  let selfChecking = $state(false);
  async function checkSelf() {
    selfChecking = true;
    try {
      const v = await call("check_self_update", {});
      if (!v) store.toast("info", "The Updater 3D is up to date.");
    } catch (e) {
      store.toast("err", `Could not check for a new version: ${e}`);
    } finally {
      selfChecking = false;
    }
  }
  let addBtn = $state<HTMLElement>();
  function addException() {
    if (!addBtn) return;
    const avail = store.apps.filter((a) => !exceptionIds.includes(a.id)).sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
    if (!avail.length) return store.toast("info", "Every app already has its own rule.");
    ui.openList({
      rect: addBtn.getBoundingClientRect(),
      items: avail.map((a) => ({ id: a.id, label: a.name })),
      width: 240,
      align: "end",
      label: "Add app",
      search: "Search apps",
      anchor: addBtn,
      onpick: (id) => setException(id, "notify"),
    });
  }

  // ----- game platforms -----
  let accounts = $state<AccountInfo[]>([]);
  /** The platform being signed in to, refreshed or signed out right now */
  let accBusy = $state<Platform | null>(null);
  /** Epic Games or GOG: its page is open in the browser, waiting for what the user pastes */
  let codeFor = $state<Platform | null>(null);
  let codeText = $state("");
  let codeError = $state<string | null>(null);
  let codeInput = $state<HTMLInputElement>();
  /** Steam's QR code while it waits for the phone (an image address) */
  let qrImage = $state<string | null>(null);
  /** Steam's sign-in window is open */
  let steamWindow = $state(false);
  onMount(() => {
    let gone = false;
    let off: (() => void) | undefined;
    on("account_qr", (img) => {
      if (qrImage) qrImage = img;
    }).then((u) => (gone ? u() : (off = u)));
    return () => {
      gone = true;
      off?.();
    };
  });
  async function loadAccounts() {
    try {
      accounts = await call("accounts", {});
    } catch (e) {
      store.toast("err", `Could not read the sign-ins: ${e}`);
    }
  }
  $effect(() => {
    if (active === "games") loadAccounts();
  });
  function accDesc(a: AccountInfo): string {
    if (!a.signedIn) return a.installsHere ? `Sign in to see every game you own on ${a.title}. They install, update and repair here, and the last update can be undone.` : `Sign in with the Steam mobile app or a Steam window to see every game you own on ${a.title}. They install and update in Steam.`;
    const who = a.name ? `Signed in as ${a.name}` : "Signed in";
    const n = `${a.games} game${a.games === 1 ? "" : "s"} in your library`;
    return `${who}. ${n}${a.syncedAt ? `, read at ${clock(a.syncedAt)}` : ""}.`;
  }
  function signedInToast(a: AccountInfo) {
    store.toast(a.problem ? "info" : "ok", a.problem ? `Signed in to ${a.title}. ${a.problem}` : `Signed in to ${a.title}. ${a.games} game${a.games === 1 ? "" : "s"} found.`);
  }
  /** Starts a sign-in: Steam shows a QR code (or, `window`, a Steam window); Epic Games and GOG open in the browser and show a code to paste here. */
  /** The newest sign-in started: an older one that ends later (a canceled QR wait) leaves the screen alone */
  let signSeq = 0;
  async function signIn(p: Platform, window = false) {
    const seq = ++signSeq;
    accBusy = p;
    codeFor = null;
    codeError = null;
    qrImage = null;
    steamWindow = p === "steam" && window;
    try {
      const step = await call("account_sign_in", { platform: p, window });
      if (step.kind === "done") signedInToast(step.account);
      else if (step.kind === "paste") {
        codeFor = p;
        codeText = "";
        tick().then(() => codeInput?.focus());
      } else {
        qrImage = step.image;
        signedInToast(await call("account_wait", { platform: p }));
      }
    } catch (e) {
      if (String(e) !== "CANCELED") store.toast("err", String(e));
    } finally {
      if (seq === signSeq) {
        accBusy = null;
        qrImage = null;
        steamWindow = false;
      }
      loadAccounts();
    }
  }
  /** From the QR code to a Steam window. */
  async function steamByWindow() {
    await call("account_cancel", { platform: "steam" });
    signIn("steam", true);
  }
  function cancelSignIn(p: Platform) {
    if (p === "steam") send("account_cancel", { platform: p });
    if (codeFor === p) codeFor = null;
  }
  async function sendCode(text = codeText) {
    const p = codeFor;
    if (!p || !text.trim() || accBusy) return;
    accBusy = p;
    codeError = null;
    try {
      const a = await call("account_code", { platform: p, text });
      codeFor = null;
      codeText = "";
      signedInToast(a);
    } catch (e) {
      codeError = String(e);
    } finally {
      accBusy = null;
      loadAccounts();
    }
  }
  /** Pasting the code signs in at once. */
  function pasted(e: ClipboardEvent) {
    const t = e.clipboardData?.getData("text") ?? "";
    if (!t.trim()) return;
    e.preventDefault();
    codeText = t.trim();
    sendCode(codeText);
  }
  const codeHelp = (p: Platform) =>
    p === "epic"
      ? "Sign in on the Epic Games page that opened in your browser. It then shows a short text with authorizationCode in it: select all of it (Ctrl A), copy it (Ctrl C) and paste it here."
      : "Sign in on the GOG page that opened in your browser. It then shows an empty page: copy that page's address from the address bar and paste it here.";
  async function refreshAccount(p: Platform) {
    accBusy = p;
    try {
      await call("account_sync", { platform: p });
    } catch (e) {
      store.toast("err", String(e));
    } finally {
      accBusy = null;
      loadAccounts();
    }
  }
  async function signOut(a: AccountInfo) {
    accBusy = a.platform;
    try {
      await call("account_sign_out", { platform: a.platform });
      store.toast("info", `Signed out of ${a.title}. Its games are no longer shown.`);
    } catch (e) {
      store.toast("err", String(e));
    } finally {
      accBusy = null;
      loadAccounts();
    }
  }

  async function changeFolder(key: "blenderInstallDir" | "adobeInstallDir" | "godotInstallDir" | "downloadDir" | "installRoot" | "gamesDir") {
    const picked = await call("pick_folder", { current: s[key] ?? "" });
    if (picked) store.patchSettings({ [key]: picked });
  }
  const freeOf = (path: string) => {
    const f = store.freeOn(driveOf(path));
    return f == null ? "" : `, ${bytes(f)} free`;
  };
  async function clearCache() {
    const n = await call("clear_cache", {});
    store.toast("ok", n > 0 ? `Freed ${bytes(n)}` : "The cache was already empty");
  }

  let testing = $state(false);
  function testNotification() {
    if (testing) return;
    testing = true;
    send("test_notification", {});
    setTimeout(() => (testing = false), 2500);
  }

  const bwOn = $derived(s.bandwidthLimitEnabled);
  const st = $derived(store.status);
</script>

<section class="screen tight set" bind:this={root}>
  <nav class="sn" aria-label="Settings pages" bind:this={navEl}>
    <h1 class="h1">Settings</h1>
    <div class="links" role="presentation" onkeydown={navKey}>
      <span class="marker" class:anim bind:this={marker} style="width:{MARKER_BASE}px"><Paint kind="swatch" fill="#202020" /></span>
      {#each nav as n (n.id)}
        <a
          bind:this={navBtns[n.id]}
          href="#/settings/{n.id}"
          class:on={active === n.id}
          aria-current={active === n.id ? "page" : undefined}
          onclick={(e) => {
            e.preventDefault();
            pick(n.id);
          }}>{n.label}</a
        >
      {/each}
    </div>
  </nav>

  <div class="stage">
    {#key active}
      <div class="body" in:pageIn={{ dir }} out:pageOut={{ dir }}>
        <header class="ph">
          <h2>{cur.label}</h2>
          <p>{cur.blurb}</p>
        </header>

        {#if active === "apps"}
          <SRow label="Adobe apps" desc={s.sources.adobe ? "Through Creative Cloud. Needs Creative Cloud installed and signed in." : "Off. Turn on to keep Adobe apps up to date."}>
            {#snippet control()}<Toggle checked={s.sources.adobe} label="Adobe apps" onchange={(v) => store.patchSettings({ sources: { ...s.sources, adobe: v } })} />{/snippet}
          </SRow>
          <SRow label="Blender" desc="Every installed version is listed. New versions install next to the old ones.">
            {#snippet control()}<Toggle checked={s.sources.blender} label="Blender" onchange={(v) => store.patchSettings({ sources: { ...s.sources, blender: v } })} />{/snippet}
          </SRow>
          {#if s.sources.blender}
            <div class="nest" in:rise>
              <SRow label="Pre-releases" desc="Alpha, beta and release candidate builds, marked Pre-release.">
                {#snippet control()}
                  <Segmented options={[{ value: "beta", label: "Show" }, { value: "stable", label: "Hide" }]} value={s.blenderChannel} onchange={(v) => store.patchSettings({ blenderChannel: v })} label="Blender pre-releases" />
                {/snippet}
              </SRow>
              <SRow label="Keep bug-fix versions" desc="Updating 5.2.2 to 5.2.3 keeps the old copy instead of replacing it." id="keep-patch">
                {#snippet control()}<Toggle checked={s.blenderKeepPatchVersions} labelledby="keep-patch" onchange={(v) => store.patchSettings({ blenderKeepPatchVersions: v })} />{/snippet}
              </SRow>
              <SRow label="Install folder" desc={s.blenderInstallDir ? s.blenderInstallDir + freeOf(s.blenderInstallDir) : "Not set"}>
                {#snippet control()}<Btn variant="raised" size="md" onclick={() => changeFolder("blenderInstallDir")}>Change</Btn>{/snippet}
              </SRow>
            </div>
          {/if}
          <SRow label="Craft apps" desc="From ArtCraft, including the ArtCraft Launcher.">
            {#snippet control()}<Toggle checked={s.sources.craft} label="Craft apps" onchange={(v) => store.patchSettings({ sources: { ...s.sources, craft: v } })} />{/snippet}
          </SRow>
          {#if s.sources.craft}
            <div class="nest" in:rise>
              <SRow label="Pre-releases" desc="Show Craft app pre-releases in Get apps." id="craft-pre">
                {#snippet control()}<Toggle checked={s.craftPrereleases} labelledby="craft-pre" onchange={(v) => store.patchSettings({ craftPrereleases: v })} />{/snippet}
              </SRow>
            </div>
          {/if}
          <SRow label="More apps" desc="OBS Studio, Krita, FreeCAD, Godot, Unity, Unity Hub and the Epic Games Launcher (Preview).">
            {#snippet control()}<Toggle checked={s.sources.tools} label="More apps: OBS Studio, Krita, FreeCAD, Godot, Unity, Unity Hub and the Epic Games Launcher (Preview)" onchange={(v) => store.patchSettings({ sources: { ...s.sources, tools: v } })} />{/snippet}
          </SRow>
          {#if s.sources.tools}
            <div class="nest" in:rise>
              <SRow label="Godot install folder" desc={s.godotInstallDir ? s.godotInstallDir + freeOf(s.godotInstallDir) : "Not set"}>
                {#snippet control()}<Btn variant="raised" size="md" onclick={() => changeFolder("godotInstallDir")}>Change</Btn>{/snippet}
              </SRow>
            </div>
          {/if}
          <SRow label="Other apps" desc="Every other app on this PC from Microsoft's winget catalog, updated with its publisher's own installer.">
            {#snippet control()}<Toggle checked={s.sources.other} label="Other apps from the winget catalog" onchange={(v) => store.patchSettings({ sources: { ...s.sources, other: v } })} />{/snippet}
          </SRow>
          <SRow label="Games" desc="Your games from Steam, Epic Games and GOG once you sign in to them in Settings, Games, and the games of Ubisoft Connect, EA app, Battle.net, Amazon Games, itch and Rockstar on this PC, with their patch notes.">
            {#snippet control()}<Toggle checked={s.sources.games !== false} label="Games" onchange={(v) => store.patchSettings({ sources: { ...s.sources, games: v } })} />{/snippet}
          </SRow>
          <SRow label="Drivers" desc="Graphics, network, audio, Bluetooth and chipset drivers from NVIDIA, AMD, Intel and Windows Update, and the newest BIOS for your board or PC (Gigabyte, ASUS, MSI, ASRock, Dell, HP and Lenovo). A BIOS is never flashed by this app.">
            {#snippet control()}<Toggle checked={s.sources.drivers !== false} label="Drivers" onchange={(v) => store.patchSettings({ sources: { ...s.sources, drivers: v } })} />{/snippet}
          </SRow>
          <SRow label="Show installed apps in Get apps" desc="Get apps also lists apps you already have, marked Installed. Search always finds them." id="show-installed">
            {#snippet control()}<Toggle checked={s.showInstalledInGetApps} labelledby="show-installed" onchange={(v) => store.patchSettings({ showInstalledInGetApps: v })} />{/snippet}
          </SRow>
        {:else if active === "general"}
          <h3 class="sub">Updates</h3>
          <SRow label="Check for updates" desc="Runs in the background, even with this window closed.">
            {#snippet control()}<Select options={intervalOpts} value={String(s.checkInterval)} onchange={(v) => store.patchSettings({ checkInterval: Number(v) })} width={180} label="Check for updates" />{/snippet}
          </SRow>
          <SRow label="Automatic updates" desc="What happens when an update is found.">
            {#snippet control()}<Segmented options={modeOpts} value={s.autoMode} onchange={(v) => store.patchSettings({ autoMode: v })} label="Automatic updates" />{/snippet}
            {#snippet below()}
              {#if s.autoMode === "install"}
                <div class="win" in:rise out:rise={{ duration: 80 }}>
                  <span>Install between</span>
                  <Select options={timeOpts(s.windowStart)} value={s.windowStart} onchange={(v) => store.patchSettings({ windowStart: v })} width={84} height={30} label="Start time" />
                  <span>and</span>
                  <Select options={timeOpts(s.windowEnd)} value={s.windowEnd} onchange={(v) => store.patchSettings({ windowEnd: v })} width={84} height={30} label="End time" />
                  <span class="note">Only while that app is closed.</span>
                </div>
              {/if}
            {/snippet}
          </SRow>
          <SRow label="App exceptions" desc="These apps follow their own rule.">
            {#snippet control()}
              <button class="add" bind:this={addBtn} onclick={addException}><Icon d={I.plus} size={14} width={2.5} />Add app</button>
            {/snippet}
            {#snippet below()}
              {#each exceptionIds as id (id)}
                {@const a = store.app(id)}
                <div class="exc" in:rise>
                  {#if a}<Mark app={a} size={24} fs={10} />{:else}<span class="ph-mark"></span>{/if}
                  <span class="en ellipsis">{a?.name ?? id}</span>
                  <Select options={ruleOpts} value={ruleOf(id)} onchange={(v) => setException(id, v as AutoMode | "ignore")} width={180} height={30} label="Rule for {a?.name ?? id}" />
                  <button class="x" aria-label="Remove exception for {a?.name ?? id}" title="Remove" onclick={() => setException(id, null)}><Icon d={I.close} size={14} width={2.5} /></button>
                </div>
              {/each}
            {/snippet}
          </SRow>
          <SRow label="Keep previous version" desc="Roll back without downloading again. Uses more disk space." id="keep-prev">
            {#snippet control()}<Toggle checked={s.keepPreviousVersions} labelledby="keep-prev" onchange={(v) => store.patchSettings({ keepPreviousVersions: v })} />{/snippet}
          </SRow>
          <SRow label="Fewer admin prompts" desc="Asks for admin rights once, then installs without a prompt each time." id="silent">
            {#snippet control()}<Toggle checked={s.silentElevation} labelledby="silent" onchange={(v) => store.patchSettings({ silentElevation: v })} />{/snippet}
          </SRow>
          <h3 class="sub">Notifications</h3>
          <SRow label="Updates found" desc="Tell me when new versions are available." id="n-upd">
            {#snippet control()}<Toggle checked={s.notifyUpdates} labelledby="n-upd" onchange={(v) => store.patchSettings({ notifyUpdates: v })} />{/snippet}
          </SRow>
          <SRow label="Install finished" desc="Tell me when an install or update is done." id="n-fin">
            {#snippet control()}<Toggle checked={s.notifyFinished} labelledby="n-fin" onchange={(v) => store.patchSettings({ notifyFinished: v })} />{/snippet}
          </SRow>
          <SRow label="Something failed" desc="Tell me when an install or update did not work." id="n-fail">
            {#snippet control()}<Toggle checked={s.notifyFailed} labelledby="n-fail" onchange={(v) => store.patchSettings({ notifyFailed: v })} />{/snippet}
          </SRow>
          <SRow label="Send a test notification" desc="Check that Windows shows notifications from The Updater 3D.">
            {#snippet control()}<Btn variant="raised" size="md" disabled={testing} onclick={testNotification}>{testing ? "Sent" : "Send test"}</Btn>{/snippet}
          </SRow>
          <p class="foot">
            Notifications come from the background checker, so they appear even when this window is closed.
            {#if !s.launchAtStartup}
              Background checking is off right now, so you will only see them while the app is open.
              <a class="lnk" href="#/settings/startup" onclick={(e) => { e.preventDefault(); pick("startup"); }}>Turn it on</a>
            {/if}
          </p>
        {:else if active === "startup"}
          <SRow label="Check for updates in the background" desc="Starts with Windows as a tiny background checker with no window. It checks on your schedule and notifies you when something can be updated." id="bg-check">
            {#snippet control()}<Toggle checked={s.launchAtStartup} labelledby="bg-check" onchange={(v) => store.patchSettings({ launchAtStartup: v })} />{/snippet}
          </SRow>
          <SRow label="Keep checking after you close the window" desc="Closing the window hides the app to the tray. Checks and installs continue." id="c2t">
            {#snippet control()}<Toggle checked={s.closeToTray} labelledby="c2t" onchange={(v) => store.patchSettings({ closeToTray: v })} />{/snippet}
          </SRow>
          <SRow label="Open in the tray when Windows starts" desc={s.launchAtStartup ? "Starts hidden in the tray instead of opening a window." : "Turn on background checking first."} dim={!s.launchAtStartup} id="tray">
            {#snippet control()}<Toggle checked={s.startMinimized && s.launchAtStartup} disabled={!s.launchAtStartup} labelledby="tray" onchange={(v) => store.patchSettings({ startMinimized: v })} />{/snippet}
          </SRow>
        {:else if active === "downloads"}
          <SRow label="Download folder" desc={s.downloadDir ? `${s.downloadDir}\\The Updater 3D downloads${freeOf(s.downloadDir)}. Old downloads in that folder are cleared by themselves.` : `In the app's own folder${freeOf("C:\\")}. Pick a folder on another drive to keep C: free.`}>
            {#snippet control()}
              {#if s.downloadDir}<Btn variant="ghost" size="md" title="Keep downloads in the app's own folder again" onclick={() => store.patchSettings({ downloadDir: "" })}>Use the default</Btn>{/if}
              <Btn variant="raised" size="md" onclick={() => changeFolder("downloadDir")}>Change</Btn>
            {/snippet}
          </SRow>
          <SRow label="Install new apps to" desc={s.installRoot ? `${s.installRoot}${freeOf(s.installRoot)}. Each new app gets its own folder there when its installer can be told; updates keep every app where it is.` : "Where each app's installer puts it, usually Program Files on C:. Pick a folder on any drive."}>
            {#snippet control()}
              {#if s.installRoot}<Btn variant="ghost" size="md" title="Let each installer choose again" onclick={() => store.patchSettings({ installRoot: "" })}>Use the default</Btn>{/if}
              <Btn variant="raised" size="md" onclick={() => changeFolder("installRoot")}>Change</Btn>
            {/snippet}
          </SRow>
          <SRow label="Parallel downloads" desc="How many apps download at the same time.">
            {#snippet control()}<Segmented options={parOpts} value={String(s.parallelDownloads)} onchange={(v) => store.patchSettings({ parallelDownloads: Number(v) })} label="Parallel downloads" />{/snippet}
          </SRow>
          <SRow label="Bandwidth limit" desc="Limits downloads only. Installs run at full speed." id="bw-limit">
            {#snippet control()}
              <Select options={bwOpts} value={String(Math.round((s.bandwidthLimit || 40e6) / 1e6))} onchange={(v) => store.patchSettings({ bandwidthLimit: Number(v) * 1e6, bandwidthLimitEnabled: true })} width={120} label="Bandwidth limit value" />
              <Toggle checked={bwOn} labelledby="bw-limit" onchange={(v) => store.patchSettings({ bandwidthLimitEnabled: v, bandwidthLimit: s.bandwidthLimit || 40e6 })} />
            {/snippet}
          </SRow>
        {:else if active === "games"}
          {#if !s.sources.games}
            <SRow label="Games are off" desc="Turn Games on in Apps to see and update your games.">
              {#snippet control()}<Btn variant="raised" size="md" onclick={() => store.patchSettings({ sources: { ...s.sources, games: true } })}>Turn on</Btn>{/snippet}
            </SRow>
          {/if}
          {#each accounts as a (a.platform)}
            <SRow label={a.title} desc={accDesc(a)}>
              {#snippet control()}
                {#if a.platform === "steam" && steamWindow}
                  <span class="ccs">Finish signing in in the Steam window</span>
                  <Btn variant="ghost" size="md" onclick={() => cancelSignIn("steam")}>Cancel</Btn>
                {:else if a.platform === "steam" && qrImage}
                  <Btn variant="ghost" size="md" onclick={() => cancelSignIn("steam")}>Cancel</Btn>
                {:else if a.signedIn}
                  {#if a.problem}<span class="ccs" style:color="var(--err)" title={a.problem}><Dab color="var(--err)" />{a.problem.startsWith("Sign in") ? "Sign in again" : "Not read"}</span>{/if}
                  {#if a.problem?.startsWith("Sign in")}
                    <Btn variant="primary" size="md" disabled={accBusy !== null} onclick={() => signIn(a.platform)}>{accBusy === a.platform ? "Signing in" : "Sign in"}</Btn>
                  {:else}
                    <Btn variant="raised" size="md" disabled={accBusy !== null} title="Read your {a.title} library again" onclick={() => refreshAccount(a.platform)}>{accBusy === a.platform ? "Reading" : "Refresh"}</Btn>
                  {/if}
                  <Btn variant="ghost" size="md" disabled={accBusy !== null} title="Forget this sign-in on this PC" onclick={() => signOut(a)}>Sign out</Btn>
                {:else if a.platform === "steam"}
                  <Btn variant="ghost" size="md" disabled={accBusy !== null} title="Sign in with your Steam password in a Steam window" onclick={() => signIn("steam", true)}>Use a Steam window</Btn>
                  <Btn variant="primary" size="md" disabled={accBusy !== null} title="Shows a QR code to scan with the Steam mobile app" onclick={() => signIn("steam")}>{accBusy === "steam" ? "Starting" : "Sign in"}</Btn>
                {:else if codeFor !== a.platform}
                  <Btn variant="primary" size="md" disabled={accBusy !== null} title="Opens {a.title}'s own sign-in page in your browser" onclick={() => signIn(a.platform)}>{accBusy === a.platform ? "Opening" : "Sign in"}<Icon d={I.external} size={13} width={2.5} /></Btn>
                {/if}
              {/snippet}
            </SRow>
            {#if a.platform === "steam" && qrImage}
              <div class="qr" in:rise>
                <img src={qrImage} alt="QR code to sign in to Steam" width="164" height="164" />
                <div class="qrt">
                  <p class="qh">Scan with the Steam mobile app</p>
                  <p>In the Steam app on your phone, open Steam Guard, tap the QR code button and scan this code. Then approve the sign-in there.</p>
                  <p>No phone with Steam? <button class="lnk" onclick={steamByWindow}>Sign in with a Steam window instead</button></p>
                </div>
              </div>
            {/if}
            {#if codeFor === a.platform}
              <div class="paste" in:rise>
                <p>{codeHelp(a.platform)}</p>
                <div class="pr">
                  <input
                    bind:this={codeInput}
                    bind:value={codeText}
                    onpaste={pasted}
                    onkeydown={(e) => e.key === "Enter" && sendCode()}
                    placeholder={a.platform === "epic" ? "Paste the text from Epic Games" : "Paste the address from GOG"}
                    aria-label={a.platform === "epic" ? "The text Epic Games shows after you sign in" : "The address GOG shows after you sign in"}
                    spellcheck="false"
                    autocomplete="off"
                  />
                  <Btn variant="primary" size="md" disabled={!codeText.trim() || accBusy !== null} onclick={() => sendCode()}>{accBusy === a.platform ? "Signing in" : "Sign in"}</Btn>
                  <Btn variant="ghost" size="md" disabled={accBusy !== null} title="Open the sign-in page in your browser again" onclick={() => signIn(a.platform)}>Open again</Btn>
                  <Btn variant="ghost" size="md" onclick={() => cancelSignIn(a.platform)}>Cancel</Btn>
                </div>
                {#if codeError}<p class="perr">{codeError}</p>{/if}
              </div>
            {/if}
          {/each}
          <SRow label="Your sign-ins" desc="You sign in on each platform's own page: Epic Games and GOG in your browser, Steam with its mobile app or in a Steam window. The Updater 3D never sees your password. What the sign-in gives back is kept encrypted for your Windows account on this PC only." />
          <SRow label="Install games to" desc={s.gamesDir ? `${s.gamesDir}${freeOf(s.gamesDir)}. Epic Games and GOG games get their own folder there. Steam installs in its own library folders.` : `A Games folder on C:${freeOf("C:\\")}. Epic Games and GOG games get their own folder there. Pick a folder on any drive.`}>
            {#snippet control()}
              {#if s.gamesDir}<Btn variant="ghost" size="md" title="Use the Games folder on C:" onclick={() => store.patchSettings({ gamesDir: "" })}>Use the default</Btn>{/if}
              <Btn variant="raised" size="md" onclick={() => changeFolder("gamesDir")}>Change</Btn>
            {/snippet}
          </SRow>
          <SRow label="Other launchers" desc="Ubisoft Connect, EA app, Battle.net, Amazon Games, itch and Rockstar have no sign-in here: their games on this PC show from the launchers' own lists and update in those launchers." />
        {:else if active === "cleanup"}
          <SRow label="Download cache" desc="Installers already used. Safe to remove. Cleared by itself after 14 days or above 5 GB, oldest first.">
            {#snippet control()}
              <span class="big">{bytes(st.cacheSize) || "0 B"}</span>
              <Btn variant="raised" size="md" title="Clear download cache" disabled={st.cacheSize === 0} onclick={clearCache}>Clear cache</Btn>
            {/snippet}
          </SRow>
          <SRow label="Dead shortcuts" desc="Shortcuts in the Start menu, on the desktop or in Startup that open programs no longer on this PC.">
            {#snippet control()}<Btn variant="raised" size="md" onclick={() => ui.openDialog({ type: "leftovers", apps: [{ id: "system:dead-shortcuts", name: "Dead shortcuts", broken: true }] })}>Find</Btn>{/snippet}
          </SRow>
          <SRow label="Logs" desc="What the app did, and the cause if it ever crashed. Attach app.log to a bug report.">
            {#snippet control()}<Btn variant="raised" size="md" onclick={() => send("open_logs_folder", {})}>Open log folder</Btn>{/snippet}
          </SRow>
        {:else if active === "adobe"}
          <SRow label="Creative Cloud" desc={st.ccInstalled ? "Installed on this PC." : "Not installed. Adobe apps need it."}>
            {#snippet control()}<span class="ccs" style:color={st.ccInstalled ? "var(--ok)" : "var(--err)"}><Dab color={st.ccInstalled ? "var(--ok)" : "var(--err)"} />{st.ccInstalled ? "Installed" : "Not installed"}</span>{/snippet}
          </SRow>
          <SRow label="Background services" desc="Sign-in and install services that Creative Cloud keeps running.">
            {#snippet control()}<span class="ccs" style:color={st.ccRunning ? "var(--ok)" : "var(--err)"}><Dab color={st.ccRunning ? "var(--ok)" : "var(--err)"} />{st.ccRunning ? "Running" : "Not running"}</span>{/snippet}
          </SRow>
          <SRow label="Sign-in" desc="This app never sees your password. It uses the session Creative Cloud keeps.">
            {#snippet control()}<span class="ccs" style:color={st.signedIn ? "var(--ok)" : "var(--err)"}><Dab color={st.signedIn ? "var(--ok)" : "var(--err)"} />{st.signedIn ? "Signed in" : "Signed out"}</span>{/snippet}
          </SRow>
          <SRow label="Open Creative Cloud" desc={s.sources.adobe ? "Sign in or start its services there." : "Adobe apps are switched off in Apps."}>
            {#snippet control()}
              <Btn variant="raised" size="md" title="Opens Creative Cloud" onclick={() => send("open_creative_cloud", {})}>Open Creative Cloud <Icon d={I.external} size={14} width={2.5} /></Btn>
            {/snippet}
          </SRow>
          <SRow label="Install language" desc="Language for new Adobe installs.">
            {#snippet control()}<Select options={langOpts} value={s.language} onchange={(v) => store.patchSettings({ language: v })} width={180} label="Language" />{/snippet}
          </SRow>
          <SRow label="Install location" desc={s.adobeInstallDir ? s.adobeInstallDir + freeOf(s.adobeInstallDir) : "Not set"}>
            {#snippet control()}<Btn variant="raised" size="md" onclick={() => changeFolder("adobeInstallDir")}>Change</Btn>{/snippet}
          </SRow>
        {:else if active === "about"}
          <SRow label="Installed version" desc={st.lastCheck ? `Last checked for updates at ${clock(st.lastCheck)}.` : "Not checked for updates yet."}>
            {#snippet control()}
              <a class="big vlink" href="#/updater" title="Open The Updater 3D's page: every release and what is new">{st.appVersion || "Unknown"}</a>
              <Btn variant="raised" size="md" title="Every release of The Updater 3D and what is new" onclick={() => router.go("#/updater")}>Version history</Btn>
            {/snippet}
          </SRow>
          <SRow label={st.selfUpdate ? `Version ${st.selfUpdate} is ready` : "You have the latest version"} desc={st.selfUpdate ? "The app restarts itself to finish the update. Running installs are not interrupted." : "The Updater 3D keeps itself up to date."}>
            {#snippet control()}
              {#if st.selfUpdate}<Btn variant="primary" size="md" disabled={ui.selfUpdating} onclick={updateSelf}>{ui.selfUpdating ? "Downloading" : "Update and restart"}</Btn>{/if}
              <Btn variant="raised" size="md" disabled={selfChecking} onclick={checkSelf} title="Looks for a new version of The Updater 3D only">{selfChecking ? "Checking" : "Check now"}</Btn>
            {/snippet}
          </SRow>
          {#if st.machine}<SRow label="Tuned for this PC" desc={`${st.machine}. Downloads and checks do as much at once as this PC handles well.`} />{/if}
        {/if}
      </div>
    {/key}
  </div>
</section>

<style>
  .set {
    flex-direction: row;
    gap: 56px;
    padding-bottom: 60px;
    align-items: flex-start;
  }
  .sn {
    position: sticky;
    top: 0;
    width: 168px;
    flex: none;
    align-self: flex-start;
    display: flex;
    flex-direction: column;
  }
  .sn .h1 {
    margin-bottom: 24px;
  }
  .links {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .links a {
    position: relative;
    height: 34px;
    display: flex;
    align-items: center;
    padding: 0 12px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-2);
    transition: color var(--t-fast);
  }
  .links a:hover,
  .links a.on {
    color: var(--text);
  }
  .marker {
    position: absolute;
    left: 0;
    top: 0;
    transform-origin: 0 0;
    opacity: 0;
    pointer-events: none;
  }
  .marker.anim {
    transition: transform 0.18s var(--ease), height 0.13s, top 0.18s var(--ease), opacity 0.13s;
  }
  /* old and new page share one grid cell while they cross-fade */
  .stage {
    flex: 1;
    min-width: 0;
    max-width: 760px;
    padding-top: 52px;
    display: grid;
  }
  .body {
    grid-area: 1 / 1;
    min-width: 0;
  }
  .ph {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 14px;
  }
  .ph h2 {
    font-size: var(--fs-20);
    line-height: 1.1;
    font-weight: 600;
    letter-spacing: var(--track-heading);
  }
  .ph p {
    font-size: 13px;
    color: var(--text-3);
  }
  /* A group title inside a page ("Updates", "Notifications"). */
  .sub {
    margin: 28px 0 4px;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-3);
  }
  .sub:first-of-type {
    margin-top: 4px;
  }
  /* Options that belong to the switch above them, shown only while it is on. */
  .nest {
    margin-left: 18px;
    padding-left: 16px;
    border-left: 2px solid var(--sep);
  }
  .foot {
    margin-top: 16px;
    padding-top: 14px;
    border-top: 1px solid var(--sep);
    font-size: 12px;
    line-height: 1.6;
    color: var(--text-3);
  }
  .lnk {
    color: var(--text);
    text-decoration: underline;
    text-underline-offset: 3px;
    text-decoration-color: var(--text-4);
  }
  .lnk:hover {
    text-decoration-color: var(--text);
  }
  .win {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    font-size: 13px;
    color: var(--text-2);
  }
  .note {
    font-size: 12px;
    color: var(--text-3);
  }
  .add {
    height: 28px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px;
    border-radius: 6px;
    color: var(--text-2);
    font-size: 12px;
    font-weight: 500;
    transition: background-color var(--t-fast), color var(--t-fast);
  }
  .add:hover {
    background: var(--surface);
    color: var(--text);
  }
  .exc {
    height: 40px;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .en {
    flex: 1;
    font-size: 13px;
  }
  .ph-mark {
    width: 24px;
    height: 24px;
  }
  .x {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    color: var(--text-3);
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background-color var(--t-fast), color var(--t-fast);
  }
  .x:hover {
    background: var(--raised);
    color: var(--text);
  }
  .big {
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .vlink {
    color: var(--text);
    text-decoration: underline;
    text-decoration-color: transparent;
    text-underline-offset: 4px;
    transition: text-decoration-color var(--t-fast);
  }
  .vlink:hover,
  .vlink:focus-visible {
    text-decoration-color: var(--text-3);
  }
  .ccs {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    font-weight: 500;
  }
  /* Steam's QR code under its row */
  .qr {
    display: flex;
    align-items: center;
    gap: 22px;
    padding: 4px 0 20px;
  }
  .qr img {
    flex: none;
    border-radius: 8px;
    background: #fff;
    padding: 6px;
    image-rendering: pixelated;
  }
  .qrt p {
    max-width: 460px;
    margin: 0 0 8px;
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-3);
  }
  .qrt .qh {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }
  .qrt .lnk {
    color: var(--text);
    text-decoration: underline;
  }
  /* what Epic Games or GOG shows after the sign-in, pasted under its row */
  .paste {
    padding: 2px 0 18px;
  }
  .paste p {
    max-width: 640px;
    margin: 0 0 10px;
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-3);
  }
  .pr {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .pr input {
    flex: 1;
    min-width: 0;
    height: 34px;
    padding: 0 12px;
    border: 0;
    border-radius: 6px;
    outline: 0;
    background: var(--surface);
    color: var(--text);
    font-size: 13px;
    transition: background-color var(--t-fast) ease;
  }
  .pr input:focus {
    background: var(--surface-2);
  }
  .pr input::placeholder {
    color: var(--text-3);
  }
  .paste .perr {
    margin: 8px 0 0;
    color: var(--err);
  }
  @media (max-width: 999px) {
    .set {
      gap: 32px;
    }
    .sn {
      width: 130px;
    }
  }
</style>
