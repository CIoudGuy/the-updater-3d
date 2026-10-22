<script lang="ts">
  import Dialog from "./Dialog.svelte";
  import Btn from "./Btn.svelte";
  import Mark from "./Mark.svelte";
  import Dab from "./Dab.svelte";
  import BrushBar from "./BrushBar.svelte";
  import Check from "./Check.svelte";
  import { store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { send } from "../lib/api";
  import { bytes, bytesParts, unitFor } from "../lib/format";
  import { SPLASH } from "../lib/paths";

  const d = $derived(ui.dialog);
  const blocked = $derived(store.blockedJobs.find((j) => !ui.dismissedBlocked.includes(j.id)));
  const kind = $derived(d ? d.type : blocked ? "blocked" : null);

  const unApp = $derived(d?.type === "uninstall" ? store.app(d.appId) : undefined);
  /** "Also delete settings and add-ons": off each time the dialog opens */
  let removePrefs = $state(false);
  $effect(() => {
    if (d?.type === "uninstall") removePrefs = false;
  });

  function doUninstall() {
    if (!unApp) return;
    send("uninstall_app", { appId: unApp.id, removePrefs });
    ui.closeDialog();
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
    <h2 id="un-title">Uninstall {unApp.name}?</h2>
    <p class="lead">
      {#if unApp.status === "missing"}
        Removes the leftover entry for {unApp.name} {unApp.version}. The files are already gone.
      {:else if unApp.source === "blender"}
        Removes {unApp.name} ({unApp.version}){unApp.sizeOnDisk ? ` and frees ${bytes(unApp.sizeOnDisk)}` : ""}. Your .blend files and other Blender versions are not touched.
      {:else}
        Removes {unApp.name} {unApp.version}{unApp.sizeOnDisk ? ` and frees ${bytes(unApp.sizeOnDisk)}` : ""}. Your projects and media files are not touched.
      {/if}
    </p>
    {#if unApp.source === "other"}
      <div class="opt info">
        <span class="grow">Runs {unApp.name}'s own uninstaller. Its settings stay where it keeps them.</span>
      </div>
    {:else}
      <div class="opt prefs">
        <Check checked={removePrefs} onchange={(v) => (removePrefs = v)}>Also delete settings and add-ons</Check>
        {#if removePrefs}
          <span class="warnline">Deletes {unApp.name}'s preferences, add-ons and caches in AppData. This can't be undone. Your own files and projects stay.</span>
        {/if}
      </div>
    {/if}
    <div class="actions first">
      <Btn variant="raised" onclick={() => ui.closeDialog()} hint="Esc">Cancel</Btn>
      <Btn variant="danger" title="Uninstall {unApp.name}" onclick={doUninstall}>Uninstall</Btn>
    </div>
  </Dialog>
{:else if kind === "blocked" && blocked}
  {@const a = store.app(blocked.appId)}
  <Dialog labelledby="blk-title" onclose={later} onenter={closeAndUpdate}>
    <svg class="splash" viewBox="0 0 300 300" aria-hidden="true"><path d={SPLASH[2]} transform="translate(80 -60)" fill={blocked.fg} fill-opacity="0.08" /></svg>
    <Mark app={a ?? blocked} size={44} fs={16} />
    <h2 id="blk-title">Close {blocked.name} to continue</h2>
    <p class="lead">
      {blocked.name} {blocked.toVersion} is downloaded and ready to install. Save your work, then close {blocked.name}.{otherJobs > 0 ? " The other updates keep going." : ""}
    </p>
    <div class="opt info">
      <span class="grow">Installs by itself when {blocked.blockedBy.length ? blocked.blockedBy.join(", ") : blocked.name} closes</span>
    </div>
    <div class="actions">
      <Btn variant="raised" onclick={later} hint="Esc">Later</Btn>
      <Btn variant="primary" onclick={closeAndUpdate} hint="Enter">Close and update</Btn>
    </div>
  </Dialog>
{:else if d?.type === "lowdisk"}
  <Dialog width={540} labelledby="disk-title" onclose={() => ui.closeDialog()}>
    <span class="eyebrow"><Dab color="#D9B26A" />Before installing {d.name}</span>
    <h2 id="disk-title" class="tight">Not enough space on {d.drive}</h2>
    <p class="lead tightp">{d.name} needs {bytes(d.needed)} on {d.drive}. It has {bytes(d.free)} free.</p>
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
