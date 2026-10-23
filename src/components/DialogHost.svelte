<script lang="ts">
  import Dialog from "./Dialog.svelte";
  import Btn from "./Btn.svelte";
  import Mark from "./Mark.svelte";
  import Dab from "./Dab.svelte";
  import BrushBar from "./BrushBar.svelte";
  import Check from "./Check.svelte";
  import LeftoverDialog from "./LeftoverDialog.svelte";
  import { store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { call, send } from "../lib/api";
  import type { PrefsItem } from "../lib/types";
  import { bytes, bytesParts, unitFor } from "../lib/format";
  import { SPLASH } from "../lib/paths";
  import { blockedText } from "../lib/blocked";

  const d = $derived(ui.dialog);
  const blocked = $derived(store.blockedJobs.find((j) => !ui.dismissedBlocked.includes(j.id)));
  const kind = $derived(d ? d.type : blocked ? "blocked" : null);

  const unApp = $derived(d?.type === "uninstall" ? store.app(d.appId) : undefined);
  /** "Also delete settings and add-ons": off each time the dialog opens */
  let removePrefs = $state(false);
  /** what ticking it would delete, read from the disk when the box is ticked */
  let prefsItems = $state<PrefsItem[] | null>(null);
  let prefsFor = "";
  $effect(() => {
    if (d?.type === "uninstall-many") manyPrefs = false;
    if (d?.type === "uninstall") {
      removePrefs = false;
      prefsItems = null;
      prefsFor = "";
    }
  });
  $effect(() => {
    if (removePrefs && unApp && prefsFor !== unApp.id) {
      const id = unApp.id;
      prefsFor = id;
      prefsItems = null;
      call("prefs_preview", { appId: id })
        .then((r) => {
          if (prefsFor === id) prefsItems = r;
        })
        .catch(() => {
          if (prefsFor === id) prefsItems = [];
        });
    }
  });
  const prefsTotal = $derived((prefsItems ?? []).reduce((n, i) => n + (i.size ?? 0), 0));

  /** A program whose uninstaller is gone: "Remove from list" instead of running it */
  const unBroken = $derived(!!unApp && unApp.source === "programs" && unApp.status === "missing");
  function doUninstall() {
    if (!unApp) return;
    const one = { id: unApp.id, name: unApp.name };
    if (unBroken) {
      ui.openDialog({ type: "leftovers", apps: [{ ...one, broken: true }] });
      return;
    }
    send("uninstall_app", { appId: unApp.id, removePrefs });
    // What the uninstaller leaves behind is shown once it is done.
    ui.openDialog({ type: "leftovers", apps: [one] });
  }
  const manyApps = $derived(d?.type === "uninstall-many" ? d.appIds.map((id) => store.app(id)).filter((a) => !!a) : []);
  const manyBytes = $derived(manyApps.reduce((n, a) => n + (a?.sizeOnDisk ?? 0), 0));
  let manyPrefs = $state(false);
  function doUninstallMany() {
    const list = manyApps.filter((a) => !!a);
    const broken = list.filter((a) => a.source === "programs" && a.status === "missing");
    for (const a of list) if (!broken.includes(a)) send("uninstall_app", { appId: a.id, removePrefs: manyPrefs });
    ui.endSelecting();
    ui.openDialog({ type: "leftovers", apps: list.map((a) => ({ id: a.id, name: a.name, broken: broken.includes(a) })) });
  }
  function later() {
    if (blocked) ui.dismissedBlocked = [...ui.dismissedBlocked, blocked.id];
  }
  function closeAndUpdate() {
    if (!blocked) return;
    send("close_and_continue", { jobId: blocked.id });
    later();
  }
  const otherJobs = $derived(store.openJobs.filter((j) => j.id !== blocked?.id).length);
  const [needN, needU] = $derived(d?.type === "lowdisk" ? bytesParts(d.needed, unitFor(d.needed)) : ["", ""]);
  const lowShort = $derived(d?.type === "lowdisk" ? Math.max(0, d.needed - d.free) : 0);
  const canClear = $derived(d?.type === "lowdisk" && d.cacheSize >= lowShort && d.cacheSize > 0);
</script>

{#if kind === "uninstall" && unApp}
  <Dialog role="alertdialog" labelledby="un-title" onclose={() => ui.closeDialog()}>
    <Mark app={unApp} size={44} fs={16} />
    <h2 id="un-title">{unBroken ? `Remove ${unApp.name} from the list?` : `Uninstall ${unApp.name}?`}</h2>
    <p class="lead">
      {#if unBroken}
        Its uninstaller is missing, so it can't run. This removes {unApp.name} from the list of installed programs and shows what is left of it to clean up. Registry entries are saved first; Restore in Activity puts them back.
      {:else if unApp.status === "missing"}
        Removes the leftover entry for {unApp.name} {unApp.version}. The files are already gone.
      {:else if unApp.source === "blender"}
        Removes {unApp.name} ({unApp.version}){unApp.sizeOnDisk ? ` and frees ${bytes(unApp.sizeOnDisk)}` : ""}. Your .blend files and other Blender versions are not touched.
      {:else}
        Removes {unApp.name} {unApp.version}{unApp.sizeOnDisk ? ` and frees ${bytes(unApp.sizeOnDisk)}` : ""}. Your projects and media files are not touched.
      {/if}
    </p>
    {#if unApp.source === "other" || (unApp.source === "programs" && !unBroken)}
      <div class="opt info">
        <span class="grow">Runs {unApp.name}'s own uninstaller{unApp.source === "programs" ? ". If it opens a window, answer it there" : ""}. Then it shows what was left behind, to clean up.</span>
      </div>
    {/if}
    {#if !unBroken}
    <div class="opt prefs">
        <Check checked={removePrefs} onchange={(v) => (removePrefs = v)}>Also delete settings and add-ons</Check>
        {#if removePrefs}
          {#if prefsItems === null}
            <span class="pnote">Checking what it would delete</span>
          {:else if prefsItems.length === 0}
            <span class="pnote">No settings or add-ons of {unApp.name} were found, so nothing else is deleted.</span>
          {:else}
            <ul class="plist selectable">
              {#each prefsItems as it (it.path)}
                <li><span class="pp mono" title={it.path}>{it.path}</span><span class="ps">{it.registry ? "Registry" : it.size ? bytes(it.size) : "Empty"}</span></li>
              {/each}
            </ul>
            <span class="warnline">{prefsTotal > 0 ? `Frees ${bytes(prefsTotal)} more. ` : ""}This can't be undone. Your own files and projects stay.</span>
          {/if}
        {/if}
    </div>
    {/if}
    <div class="actions first">
      <Btn variant="raised" onclick={() => ui.closeDialog()} hint="Esc">Cancel</Btn>
      <Btn variant="danger" title="{unBroken ? 'Remove' : 'Uninstall'} {unApp.name}" onclick={doUninstall}>{unBroken ? "Continue" : "Uninstall"}</Btn>
    </div>
  </Dialog>
{:else if d?.type === "uninstall-many"}
  <Dialog role="alertdialog" labelledby="unm-title" width={520} onclose={() => ui.closeDialog()}>
    <h2 id="unm-title">Uninstall {manyApps.length} programs?</h2>
    <p class="lead">
      They are removed one after another{manyBytes ? `, freeing ${bytes(manyBytes)}` : ""}. Programs with their own uninstaller window wait for your answer there. Then it shows what each left behind, to clean up.
    </p>
    <ul class="plist many selectable">
      {#each manyApps as a (a?.id)}
        {#if a}<li><span class="pp">{a.name}{a.source === "programs" && a.status === "missing" ? " (removed from the list: its uninstaller is missing)" : ""}</span><span class="ps">{a.sizeOnDisk ? bytes(a.sizeOnDisk) : ""}</span></li>{/if}
      {/each}
    </ul>
    <div class="opt prefs">
      <Check checked={manyPrefs} onchange={(v) => (manyPrefs = v)}>Also delete their settings and add-ons</Check>
    </div>
    <div class="actions first">
      <Btn variant="raised" onclick={() => ui.closeDialog()} hint="Esc">Cancel</Btn>
      <Btn variant="danger" onclick={doUninstallMany}>Uninstall {manyApps.length}</Btn>
    </div>
  </Dialog>
{:else if d?.type === "leftovers"}
  {#key d}
    <LeftoverDialog apps={d.apps} />
  {/key}
{:else if kind === "blocked" && blocked}
  {@const a = store.app(blocked.appId)}
  <Dialog labelledby="blk-title" onclose={later} onenter={closeAndUpdate}>
    <svg class="splash" viewBox="0 0 300 300" aria-hidden="true"><path d={SPLASH[2]} transform="translate(80 -60)" fill={blocked.fg} fill-opacity="0.08" /></svg>
    <Mark app={a ?? blocked} size={44} fs={16} />
    {@const t = blockedText(blocked, otherJobs)}
    <h2 id="blk-title">{t.title}</h2>
    <p class="lead">{t.lead}</p>
    <div class="opt info">
      <span class="grow">{t.info}</span>
    </div>
    <div class="actions">
      <Btn variant="raised" onclick={later} hint="Esc">Later</Btn>
      <Btn variant="primary" onclick={closeAndUpdate} hint="Enter">{t.action}</Btn>
    </div>
  </Dialog>
{:else if d?.type === "lowdisk"}
  <Dialog width={540} labelledby="disk-title" onclose={() => ui.closeDialog()} onenter={d.relocate ? d.onRelocate : undefined}>
    <span class="eyebrow"><Dab color="#D9B26A" />{d.eyebrow}</span>
    <h2 id="disk-title" class="tight">Not enough space on {d.drive}</h2>
    <p class="lead tightp">{d.name} {d.needs} {bytes(d.needed)} on {d.drive}. It has {bytes(d.free)} free.</p>
    <div class="stats">
      <div><span class="k">Needed</span><span class="v">{needN} <span class="u">{needU}</span></span></div>
      <div><span class="k">Free on {d.drive}</span><span class="v">{bytesParts(d.free, unitFor(d.needed))[0]} <span class="u">{needU}</span></span></div>
      <div><span class="k">Short by</span><span class="v warn">{bytesParts(lowShort, unitFor(d.needed))[0]} <span class="u">{needU}</span></span></div>
    </div>
    <div class="space"><BrushBar value={Math.min(1, d.free / Math.max(1, d.needed))} color="#EDEDED" opacity={1} height={8} track="#41381f" shine={false} /></div>
    <div class="choices">
      {#if canClear}
        <div class="choice">
          <div class="grow"><span class="t">Clear the download cache</span><span class="s">Frees {bytes(d.cacheSize)} on {d.drive}, then installs there.</span></div>
          <Btn variant="raised" size="md" title="Clear cache and install" onclick={d.onClearCache}>Clear cache</Btn>
        </div>
      {/if}
      {#if d.relocate}
        <div class="choice">
          <div class="grow"><span class="t">Install on {d.relocate.drive} instead</span><span class="s">{bytes(d.relocate.free)} free.</span></div>
          <Btn variant="primary" size="md" title="Install on {d.relocate.drive}  Enter" onclick={d.onRelocate}>Install on {d.relocate.drive}</Btn>
        </div>
      {/if}
      {#if !canClear && !d.relocate}
        <div class="choice">
          <div class="grow"><span class="t">Free up space on {d.drive}</span><span class="s">Remove files you no longer need, then try again.</span></div>
        </div>
      {/if}
    </div>
    <div class="cancel"><Btn variant="ghost" size="md" onclick={() => ui.closeDialog()}>Cancel</Btn></div>
  </Dialog>
{/if}

<style>
  h2 {
    margin: 20px 0 0;
    font-size: 20px;
    line-height: 1.25;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  h2.tight {
    margin-top: 12px;
  }
  .lead {
    margin: 10px 0 0;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-2);
  }
  .lead.tightp {
    margin-top: 8px;
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 16px;
    margin-top: 20px;
    padding: 12px 0;
    border-top: 1px solid var(--sep-raised);
    font-size: 13px;
  }
  .opt.info {
    color: var(--text-2);
  }
  .opt.prefs {
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
  }
  .plist.many {
    margin-top: 16px;
    max-height: 220px;
  }
  .pnote {
    font-size: 12px;
    color: var(--text-3);
  }
  .plist {
    width: 100%;
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 160px;
    overflow-y: auto;
  }
  .plist li {
    display: flex;
    gap: 12px;
    align-items: baseline;
    font-size: 12px;
  }
  .pp {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-2);
  }
  .ps {
    flex: none;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .warnline {
    font-size: 12px;
    line-height: 1.45;
    color: var(--warn);
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .actions.first {
    margin-top: 24px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
  .splash {
    position: absolute;
    right: -28px;
    top: -28px;
    width: 220px;
    height: 160px;
    overflow: hidden;
    pointer-events: none;
  }
  .eyebrow {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--warn);
  }
  .stats {
    display: flex;
    gap: 40px;
    margin-top: 22px;
  }
  .stats div {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .k {
    font-size: 12px;
    color: var(--text-3);
  }
  .v {
    font-size: 28px;
    line-height: 1;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .v .u {
    font-weight: 500;
    color: var(--text-3);
  }
  .v.warn,
  .v.warn .u {
    color: var(--warn);
  }
  .space {
    margin-top: 16px;
  }
  .choices {
    display: flex;
    flex-direction: column;
    margin-top: 22px;
  }
  .choice {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 12px 0;
    border-top: 1px solid var(--sep-raised);
  }
  .choice .t {
    display: block;
    font-size: 13px;
    font-weight: 500;
  }
  .choice .s {
    display: block;
    margin-top: 2px;
    font-size: 12px;
    color: var(--text-3);
  }
  .cancel {
    display: flex;
    justify-content: flex-end;
    margin-top: 8px;
  }
</style>
