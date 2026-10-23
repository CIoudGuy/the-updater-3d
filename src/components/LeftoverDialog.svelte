<script lang="ts">
  // After an uninstall (or for "Remove from list"): waits for the uninstallers, then shows what
  // they left behind with sizes. Nothing is deleted until "Delete selected"; registry entries,
  // services and tasks are saved first and can be restored from Activity for 30 days.
  import Dialog from "./Dialog.svelte";
  import Btn from "./Btn.svelte";
  import Check from "./Check.svelte";
  import { store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { call } from "../lib/api";
  import { bytes } from "../lib/format";
  import { jobStatus } from "../lib/jobs";
  import type { LeftoverItem, LeftoverKind, LeftoverResult, LeftoverScan } from "../lib/types";

  let { apps }: { apps: { id: string; name: string; broken?: boolean }[] } = $props();

  type St = "waiting" | "failed" | "scanning" | "ready" | "deleting" | "done";
  interface Entry {
    id: string;
    name: string;
    broken: boolean;
    state: St;
    scan: LeftoverScan | null;
    error: string | null;
    result: LeftoverResult | null;
  }
  // svelte-ignore state_referenced_locally
  let entries = $state<Entry[]>(apps.map((a) => ({ id: a.id, name: a.name, broken: !!a.broken, state: "waiting", scan: null, error: null, result: null })));
  /** ticked items per scan */
  let picked = $state<Record<string, string[]>>({});
  const started = Date.now();
  // Uninstalls that had already finished before this dialog opened (an earlier uninstall of the
  // same app) must not count as this one being done.
  // svelte-ignore state_referenced_locally
  const earlier = new Set(store.jobs.filter((j) => j.kind === "uninstall" && (j.state === "done" || j.state === "failed" || j.state === "canceled")).map((j) => j.id));

  const KIND: Record<LeftoverKind, string> = {
    folder: "Install folder",
    entry: "Installed programs entry",
    shortcut: "Shortcut",
    startup: "Starts with Windows",
    registry: "Registry key",
    service: "Service",
    task: "Scheduled task",
    appPath: "App path",
    fileType: "File type",
    settings: "Settings and add-ons",
  };

  function scan(e: Entry) {
    e.state = "scanning";
    call("leftover_scan", { appId: e.id })
      .then((s) => {
        e.scan = s;
        e.state = "ready";
        picked[s.scanId] = s.items.filter((i) => i.checked || i.required).map((i) => i.id);
      })
      .catch((err) => {
        e.state = "failed";
        e.error = String(err?.message ?? err);
      });
  }

  // Broken programs are scanned at once; the others when their uninstall is done.
  $effect(() => {
    for (const e of entries) {
      if (e.state !== "waiting") continue;
      if (e.broken) {
        scan(e);
        continue;
      }
      const job = store.jobs.filter((j) => j.appId === e.id && j.kind === "uninstall" && !earlier.has(j.id)).at(-1);
      if (job?.state === "done" || (!job && !store.app(e.id) && Date.now() - started > 1500)) scan(e);
      else if (job && (job.state === "failed" || job.state === "canceled")) {
        e.state = "failed";
        e.error = job.state === "canceled" ? "The uninstall was canceled." : (job.error?.message ?? "The uninstall did not finish.");
      }
    }
  });

  const waiting = $derived(entries.filter((e) => e.state === "waiting" || e.state === "scanning").length);
  const ready = $derived(entries.filter((e) => e.state === "ready" && e.scan && e.scan.items.length > 0));
  const chosen = $derived(ready.flatMap((e) => (e.scan ? e.scan.items.filter((i) => (picked[e.scan!.scanId] ?? []).includes(i.id)) : [])));
  const chosenBytes = $derived(chosen.reduce((n, i) => n + (i.kind === "folder" || i.kind === "shortcut" || i.kind === "startup" || i.kind === "settings" ? (i.size ?? 0) : 0), 0));
  const onlyBroken = $derived(entries.every((e) => e.broken));
  const deleting = $derived(entries.some((e) => e.state === "deleting"));
  const allDone = $derived(entries.length > 0 && entries.every((e) => e.state === "done" || e.state === "failed" || (e.state === "ready" && e.scan?.items.length === 0)));

  function toggle(scanId: string, it: LeftoverItem, on: boolean) {
    if (it.required) return;
    const cur = picked[scanId] ?? [];
    picked[scanId] = on ? [...cur, it.id] : cur.filter((x) => x !== it.id);
  }
  function job(e: Entry) {
    return store.jobs.filter((j) => j.appId === e.id && j.kind === "uninstall" && !earlier.has(j.id)).at(-1);
  }

  async function del() {
    for (const e of ready) {
      const s = e.scan!;
      const ids = picked[s.scanId] ?? [];
      if (!ids.length) continue;
      e.state = "deleting";
      try {
        e.result = await call("leftover_delete", { scanId: s.scanId, ids });
        e.state = "done";
      } catch (err) {
        e.state = "failed";
        e.error = String((err as Error)?.message ?? err);
      }
    }
    const freed = entries.reduce((n, e) => n + (e.result?.freed ?? 0), 0);
    const removed = entries.reduce((n, e) => n + (e.result?.removed ?? 0), 0);
    if (removed) store.toast("ok", freed > 0 ? `Cleaned up, ${bytes(freed)} freed` : "Cleaned up");
  }
  // The PC-wide shortcut cleanup (Settings) uses this dialog too.
  const deadLinks = $derived(entries.length === 1 && entries[0].id === "system:dead-shortcuts");
  const title = $derived(deadLinks ? "Shortcuts to programs that are gone" : onlyBroken ? `Remove ${entries.length === 1 ? entries[0].name : `${entries.length} programs`} from the list` : `Cleaning up after ${entries.length === 1 ? entries[0].name : `${entries.length} programs`}`);
</script>

<Dialog width={600} labelledby="lo-title" onclose={() => !deleting && ui.closeDialog()}>
  <h2 id="lo-title">{title}</h2>
  <p class="lead">
    {#if deadLinks}
      {waiting ? "Looking through the Start menu, desktops and Startup." : "These open programs that are no longer on this PC. Shortcuts to an unplugged drive or a network share are left alone."}
    {:else if waiting && !onlyBroken}
      Waiting for the uninstaller{entries.length > 1 ? "s" : ""}. If one opens its own window, answer it there. Then this shows what was left behind.
    {:else if onlyBroken}
      Its uninstaller is gone, so it can not run. This removes the entry and what is left of the program. Tick what to delete.
    {:else}
      Tick what to delete. Registry entries, services and tasks are saved first: Restore in Activity puts them back for 30 days.
    {/if}
  </p>
  <div class="list">
    {#each entries as e (e.id)}
      <section class="app">
        <h3>{e.name}</h3>
        {#if e.state === "waiting"}
          {@const j = job(e)}
          <span class="pnote">{j ? (j.state === "installing" || j.state === "verifying" ? j.step || "Uninstalling" : jobStatus(j, false)) : "Uninstalling"}</span>
        {:else if e.state === "scanning"}
          <span class="pnote">Looking for what it left behind</span>
        {:else if e.state === "failed"}
          <span class="pnote err">{e.error}</span>
        {:else if e.state === "deleting"}
          <span class="pnote">Deleting</span>
        {:else if e.state === "done" && e.result}
          <span class="pnote">
            {e.result.removed} removed{e.result.freed ? `, ${bytes(e.result.freed)} freed` : ""}.{e.result.backupId ? " Restore in Activity puts the registry entries back." : ""}
          </span>
          {#each e.result.failed as f (f)}<span class="pnote err">{f}</span>{/each}
        {:else if e.scan && e.scan.items.length === 0}
          <span class="pnote">Nothing was left behind.</span>
        {:else if e.scan}
          <ul class="plist selectable">
            {#each e.scan.items as it (it.id)}
              {@const on = (picked[e.scan.scanId] ?? []).includes(it.id)}
              <li>
                <Check checked={on} onchange={(v) => toggle(e.scan!.scanId, it, v)}>
                  <span class="it">
                    <span class="k">{KIND[it.kind]}{it.required ? ", always removed" : ""}{it.elevated ? ", needs admin" : ""}</span>
                    <span class="pp mono" title={it.label}>{it.label}</span>
                    <span class="why">{it.detail}</span>
                  </span>
                </Check>
                <span class="ps">{it.size ? bytes(it.size) : it.kind === "folder" ? "Empty" : ""}</span>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/each}
  </div>
  {#if chosen.length && !allDone}
    <span class="warnline">{chosen.length} selected{chosenBytes ? `, frees ${bytes(chosenBytes)}` : ""}. Files and folders can't be restored; your own files and projects are never listed.</span>
  {/if}
  <div class="actions">
    <Btn variant="raised" onclick={() => ui.closeDialog()} disabled={deleting} hint="Esc">{allDone ? "Close" : "Skip"}</Btn>
    {#if !allDone}
      <Btn variant="danger" disabled={!chosen.length || deleting || waiting > 0} onclick={del}>{deadLinks ? "Delete selected" : onlyBroken ? "Remove from list" : "Delete selected"}</Btn>
    {/if}
  </div>
</Dialog>

<style>
  h2 {
    margin: 0;
    font-size: 20px;
    line-height: 1.25;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  h3 {
    margin: 0 0 6px;
    font-size: 13px;
    font-weight: 600;
  }
  .lead {
    margin: 10px 0 0;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-2);
  }
  .list {
    margin-top: 16px;
    max-height: 380px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .app {
    padding-top: 12px;
    border-top: 1px solid var(--sep-raised);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .pnote {
    font-size: 12px;
    color: var(--text-3);
  }
  .pnote.err {
    color: var(--err);
  }
  .plist {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .plist li {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    justify-content: space-between;
  }
  .it {
    display: flex;
    flex-direction: column;
    min-width: 0;
    max-width: 440px;
  }
  .k {
    font-size: 11px;
    color: var(--text-3);
  }
  .pp {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-2);
  }
  .why {
    font-size: 11px;
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ps {
    flex: none;
    font-size: 12px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .warnline {
    display: block;
    margin-top: 12px;
    font-size: 12px;
    line-height: 1.45;
    color: var(--warn);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
</style>
