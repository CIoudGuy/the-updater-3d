<script lang="ts">
  import { flip } from "svelte/animate";
  import Statement from "../components/Statement.svelte";
  import Splash from "../components/Splash.svelte";
  import Btn from "../components/Btn.svelte";
  import Mark from "../components/Mark.svelte";
  import Dab from "../components/Dab.svelte";
  import Icon from "../components/Icon.svelte";
  import Num from "../components/Num.svelte";
  import BrushBar from "../components/BrushBar.svelte";
  import { BRUSH, MARK } from "../lib/paths";
  import { I } from "../lib/icons";
  import { call, send } from "../lib/api";
  import { store } from "../lib/store.svelte";
  import { canRepair, ui } from "../lib/ui.svelte";
  import { router, appHref } from "../lib/router.svelte";
  import { checkNow, repairApp, togglePauseAll, updateAll, updateOne, isPaused, updateSelf } from "../lib/actions";
  import { bytes, checkedLine, clock, countOf, duration, inUnit, nameList, splitVersion, unitFor } from "../lib/format";
  import { NO_STOP_TIP, canPause, jobStatus, pauseTip, pct } from "../lib/jobs";
  import { enter, reduced, rise } from "../lib/motion";
  import type { ServiceInfo } from "../lib/types";

  type HState = "first" | "ccdown" | "signedout" | "offline" | "updating" | "failed" | "ready" | "repair" | "current";
  const hs = $derived.by((): HState => {
    const s = store.status;
    if (s.firstScan) return "first";
    if (store.ccDown) return "ccdown";
    if (store.signedOut) return "signedout";
    if (!s.online) return "offline";
    if (store.openJobs.length > 0 && store.updates.length === 0) return "updating";
    if (store.openJobs.length === 0 && store.failedJobs.length > 0) return "failed";
    if (store.updates.length > 0) return "ready";
    if (store.missing.length > 0) return "repair";
    return "current";
  });

  const updates = $derived(store.updates);
  const colors = $derived.by(() => {
    const src = hs === "updating" ? store.openJobs.map((j) => j.fg) : hs === "ready" || hs === "offline" ? updates.map((a) => a.fg) : hs === "repair" || hs === "failed" ? [...store.failedJobs.map((j) => j.fg), ...store.missing.map((a) => a.fg)] : [];
    return src.slice(0, 3);
  });
  const splashColors = $derived.by(() => {
    switch (hs) {
      case "current":
        return ["#7FB77E", "#7FB77E", "#7FB77E"];
      case "ccdown":
      case "failed":
        return ["#D9706A", colors[0] ?? "#D9706A", colors[1] ?? "#D9706A"];
      case "offline":
      case "signedout":
        return ["#A0A0A0", "#A0A0A0", "#A0A0A0"];
      case "first":
        return [];
      default:
        return colors.length ? colors : ["#A0A0A0"];
    }
  });
  const splashOpacity = $derived(hs === "offline" || hs === "signedout" ? 0.05 : hs === "current" ? 0.07 : 0.08);

  // ----- statement text -----
  const stmt = $derived.by(() => {
    const n = updates.length;
    switch (hs) {
      case "ready":
        return { pre: "", word: countOf(n, "update"), post: " ready", color: "#D9B26A", op: 0.5 };
      case "updating": {
        const batch = store.batchJobs;
        const verb = batch.every((j) => j.kind === "update") ? "Updating " : batch.every((j) => j.kind === "install") ? "Installing " : "Working on ";
        return { pre: verb, word: countOf(batch.length, "app"), post: "", color: "#7C93D6", op: 0.42 };
      }
      case "current":
        return { pre: "Everything is ", word: "current", post: "", color: "#7FB77E", op: 0.45 };
      case "offline":
        return { pre: "You are ", word: "offline", post: "", color: "#D9706A", op: 0.42 };
      case "failed": {
        const f = store.failedJobs.length;
        return { pre: `${f} ${f === 1 ? "update" : "updates"} `, word: "failed", post: "", color: "#D9706A", op: 0.45 };
      }
      case "repair": {
        const m = store.missing.length;
        return { pre: "", word: countOf(m, "app"), post: m === 1 ? " needs repair" : " need repair", color: "#D9706A", op: 0.45 };
      }
      case "ccdown":
        return { pre: "Creative Cloud services are ", word: "not running", post: "", color: "#D9706A", op: 0.42 };
      case "signedout":
        return { pre: "You are ", word: "signed out", post: "", color: "#A0A0A0", op: 0.3 };
      default:
        return { pre: "", word: "", post: "", color: "#A0A0A0", op: 0.3 };
    }
  });
  const bigStmt = $derived(hs === "ccdown" ? 56 : 64);

  const lede = $derived.by(() => {
    const s = store.status;
    switch (hs) {
      case "ready": {
        const names = updates.map((a) => a.name);
        return store.updatesBytes > 0 ? `${bytes(store.updatesBytes)} to download for ${nameList(names)}.` : `Updates for ${nameList(names)}.`;
      }
      case "current": {
        const base = `${countOf(store.apps.length, "app")} checked at ${clock(s.lastCheck)}.`;
        const h = store.settings.checkInterval;
        if (!h || !s.lastCheck) return base + " Automatic checks are off.";
        return `${base} The next check runs at ${clock(new Date(+new Date(s.lastCheck) + h * 36e5))}.`;
      }
      case "offline":
        return `This is what was known at ${clock(s.lastCheck)}. Checks and downloads start again when the connection is back.`;
      case "failed": {
        const j = store.failedJobs[0];
        const others = store.jobs.filter((x) => x.state === "done").length;
        const why = j.error ? `${j.error.message} ` : "";
        return `${j.name} ${j.toVersion} could not install. ${why}${j.fromVersion ? `Nothing changed, ${j.fromVersion} still works.` : ""}${others ? ` The other ${others === 1 ? "update" : `${others} updates`} finished.` : ""}`;
      }
      case "repair": {
        const m = store.missing;
        return m.length === 1 ? `${m[0].name} ${m[0].version} is registered as installed but its files are gone. Repair reinstalls it.` : `${nameList(m.map((a) => a.name))} are registered as installed but their files are gone. Repair reinstalls them.`;
      }
      case "ccdown":
        return "Sign-in, licensing and installs go through them. Your apps are still installed. Start the services to check for updates again.";
      case "signedout":
        return "Sign in through Creative Cloud. This app never sees your password, it uses the session Creative Cloud keeps.";
      default:
        return "";
    }
  });

  // ----- extras -----
  let svc = $state<ServiceInfo[]>([]);
  $effect(() => {
    if (hs === "ccdown") call("get_services", {}).then((r) => (svc = r.filter((x) => x.required)));
  });
  const otherUpdates = $derived(updates.filter((a) => a.source !== "adobe").length);

  const doneJobs = $derived(store.jobs.filter((j) => j.state === "done" || j.state === "failed"));

  function review() {
    ui.filter = "updates";
    ui.group = "all";
    router.go("installed");
  }
  function retryFailed() {
    for (const j of store.failedJobs) send("retry_job", { jobId: j.id });
  }
  function pauseJob(id: string, state: string) {
    send(state === "paused" ? "resume_job" : "pause_job", { jobId: id });
  }
  const paused = $derived(isPaused());
  const dlU = $derived(unitFor(store.totalBytes));
</script>

<section class="screen home">
  <Splash colors={splashColors} opacity={splashOpacity} lines={hs !== "first"} />

  <div class="top">
    {#if hs === "first"}
      <span class="lt t2">{store.status.scanStep ?? "Reading installed apps"}</span>
    {:else if hs === "offline"}
      <span class="lt err"><Dab color="#D9706A" />No connection</span>
    {:else if hs === "ccdown"}
      <span class="lt err"><Dab color="#D9706A" />Checks and installs are paused</span>
    {:else if hs === "signedout"}
      <span class="lt"><Dab color="#6E6E6E" />No Adobe account</span>
    {:else}
      <span class="lt">
        {#if hs === "current"}<Dab color="#7FB77E" />{/if}
        <Num value={store.apps.length} /> {store.apps.length === 1 ? "app" : "apps"} installed
      </span>
    {/if}
    {#if hs !== "ccdown" && hs !== "signedout"}
      <div class="chk">
        {#if hs === "offline"}
          <span class="sub">Last checked at {clock(store.status.lastCheck)}</span>
        {:else if hs === "failed"}
          <span class="sub">Finished at {clock(store.jobs.map((j) => j.finishedAt).filter(Boolean).sort().pop() ?? null)}</span>
        {:else if hs !== "first"}
          <span class="sub">{checkedLine(store.status.lastCheck, store.now)}</span>
        {/if}
        {#if hs !== "offline"}
          <Btn variant="ghost" size="md" title="Check now  F5" onclick={checkNow} disabled={hs === "first" || store.status.checking}>
            <Icon d={I.refresh} class={store.status.checking ? "spin" : ""} />
            {store.status.checking ? "Checking" : "Check now"}
          </Btn>
        {/if}
      </div>
    {/if}
  </div>

  {#if store.status.selfUpdate && hs !== "first"}
    <div class="selfup" in:rise>
      <Dab color="#7C93D6" />
      <span>The Updater 3D {store.status.selfUpdate} is ready.</span>
      <button class="lnk" disabled={ui.selfUpdating} onclick={updateSelf}>{ui.selfUpdating ? "Downloading, restarts by itself" : "Update and restart"}</button>
    </div>
  {/if}

  {#if hs === "first"}
    <div class="skel-head" aria-hidden="true">
      <svg viewBox="0 0 400 24" preserveAspectRatio="none" style="width:540px;height:58px;margin-top:76px"><path d={BRUSH} fill="#1F1F1F" /></svg>
      <svg viewBox="0 0 400 24" preserveAspectRatio="none" style="width:390px;height:16px;margin-top:22px"><path d={BRUSH} fill="#1F1F1F" /></svg>
      <div class="skel-btns">
        <svg viewBox="0 0 120 36" preserveAspectRatio="none" style="width:124px;height:36px"><path d="M5 3.2C30 1.8 80 1.6 114 2.8c3 .1 4.8 2.4 5 5.6.4 6.4.4 13.4 0 19.6-.2 3.4-2 5.4-5.2 5.6-30 1.6-78 1.8-108 .4C2.6 33.8 1 31.8.8 28.6.4 22 .4 14.4.9 8 1.1 5.2 2.6 3.4 5 3.2z" fill="#1F1F1F" /></svg>
        <svg viewBox="0 0 120 36" preserveAspectRatio="none" style="width:136px;height:36px"><path d="M5 3.2C30 1.8 80 1.6 114 2.8c3 .1 4.8 2.4 5 5.6.4 6.4.4 13.4 0 19.6-.2 3.4-2 5.4-5.2 5.6-30 1.6-78 1.8-108 .4C2.6 33.8 1 31.8.8 28.6.4 22 .4 14.4.9 8 1.1 5.2 2.6 3.4 5 3.2z" fill="#1F1F1F" /></svg>
      </div>
    </div>
    <div class="grid4 first">
      {#each store.apps as a, i (a.id)}
        <div class="gtile" use:enter={i}>
          <Mark app={a} size={32} />
          <div class="gcol"><span class="gname ellipsis">{a.name}</span><span class="gver">{a.version}</span></div>
        </div>
      {/each}
      {#each [0, 1, 2, 3] as i (i)}
        <div class="gtile skel" aria-hidden="true" style="animation-delay:{i * 120}ms">
          <svg viewBox="0 0 48 48" width="32" height="32"><path d={MARK} fill="#1F1F1F" /></svg>
          <div class="gcol" style="gap:10px;padding-top:2px">
            <svg viewBox="0 0 400 24" preserveAspectRatio="none" style="width:100px;height:11px"><path d={BRUSH} fill="#1F1F1F" /></svg>
            <svg viewBox="0 0 400 24" preserveAspectRatio="none" style="width:68px;height:24px"><path d={BRUSH} fill="#1F1F1F" /></svg>
          </div>
        </div>
      {/each}
    </div>
  {:else}
    <div class="statement-wrap" class:wide={hs === "ccdown"}>
      <Statement pre={stmt.pre} word={stmt.word} post={stmt.post} color={stmt.color} opacity={stmt.op} size={bigStmt} />
    </div>

    {#if hs === "updating"}
      <div class="stats big">
        <div class="stat">
          <span class="k">Downloaded</span>
          <span class="v"><Num value={inUnit(store.downloaded, dlU)} fmt={(n) => n.toFixed(1)} /> <span class="u">of {bytes(store.totalBytes)}</span></span>
        </div>
        <div class="stat">
          <span class="k">Speed</span>
          <span class="v"><Num value={store.speed / 1e6} fmt={(n) => String(Math.round(n))} /> <span class="u">MB/s</span></span>
        </div>
        <div class="stat">
          <span class="k">Time left</span>
          <span class="v">{store.eta == null ? "" : duration(store.eta)}</span>
        </div>
      </div>
    {:else if lede}
      <p class="lede">{lede}</p>
    {/if}

    <div class="actions">
      {#if hs === "ready"}
        <Btn variant="primary" hint="Ctrl U" title="Update all  Ctrl U" onclick={updateAll}>Update all</Btn>
        <Btn variant="secondary" title="Review in Installed  Ctrl 2" onclick={review}>Review updates</Btn>
      {:else if hs === "updating"}
        <Btn variant="secondary" hint="Ctrl P" title={store.openJobs.some(canPause) ? (paused ? "Resume all  Ctrl P" : "Pause all  Ctrl P") : NO_STOP_TIP} disabled={!store.openJobs.some(canPause)} onclick={togglePauseAll}>{paused ? "Resume all" : "Pause all"}</Btn>
        <Btn variant="ghost" href="#/queue" title="Open queue  Ctrl 4">Open queue</Btn>
      {:else if hs === "offline"}
        <Btn variant="secondary" hint="F5" title="Try again  F5" onclick={checkNow}>Try again</Btn>
      {:else if hs === "failed"}
        <Btn variant="primary" hint="R" title="Retry  R" onclick={retryFailed}>Retry</Btn>
        <Btn variant="secondary" href="#/activity" title="View log  L">View log</Btn>
        <Btn variant="ghost" onclick={() => send("clear_finished_jobs", {})}>Dismiss</Btn>
      {:else if hs === "repair"}
        <Btn variant="primary" onclick={() => store.missing.filter(canRepair).forEach((a) => repairApp(a.id))}>{store.missing.length === 1 ? "Repair" : "Repair all"}</Btn>
        <Btn variant="ghost" href={appHref(store.missing[0].id)}>Open details</Btn>
      {:else if hs === "ccdown"}
        <Btn variant="primary" hint="Enter" title="Start services  Enter" onclick={() => send("open_creative_cloud", {})}>Start services</Btn>
        <Btn variant="ghost" href="#/services" title="View services  Ctrl 6">View services</Btn>
      {:else if hs === "signedout"}
        <Btn variant="primary" title="Open Creative Cloud to sign in  Enter" onclick={() => send("open_creative_cloud", {})}>
          Open Creative Cloud to sign in <Icon d={I.external} size={14} width={2.5} />
        </Btn>
      {/if}
    </div>

    {#if hs === "ready" || hs === "offline"}
      <div class="cols" class:single={hs === "offline"}>
        <section aria-label={hs === "offline" ? "Found before going offline" : "Ready to update"}>
          <div class="sec-h">
            {#if hs === "offline"}
              <span>{countOf(updates.length, "update")} found before you went offline</span>
            {:else}
              <span class="ttl"><Dab color="#D9B26A" />Ready to update</span>
            {/if}
            <span class="aside">{bytes(store.updatesBytes)}</span>
          </div>
          {#each updates as a, i (a.id)}
            <div class="row" animate:flip={{ duration: reduced() ? 0 : 220 }} use:enter={i}>
              <Mark app={a} size={32} dim={hs === "offline"} />
              <div class="rcol">
                <span class="rname ellipsis">{a.name}</span>
                <span class="rsub ellipsis" title={a.note ?? ""}>{a.version}  →  {a.latest}{a.note ? `, ${a.note}` : ""}</span>
              </div>
              {#if a.updateSize}<span class="rsize">{bytes(a.updateSize)}</span>{/if}
              {#if hs === "offline"}
                <span class="rsub">Waiting for connection</span>
              {:else}
                <Btn variant="raised" size="sm" title="Update {a.name}  U" onclick={() => updateOne(a.id)}>Update</Btn>
              {/if}
            </div>
          {/each}
        </section>
        {#if hs === "ready"}
          <section aria-label="Updating now" class="nowcol">
            <div class="sec-h"><span>Updating now</span></div>
            {#if store.openJobs.length === 0}
              <div class="sep-top empty">Nothing is updating. Jobs you start show here with live progress.</div>
            {:else}
              {#each store.openJobs as j, i (j.id)}
                <div class="jrow" animate:flip={{ duration: reduced() ? 0 : 220 }} use:enter={i}>
                  <Mark app={j} size={28} />
                  <div class="rcol">
                    <span class="rname ellipsis">{j.name}</span>
                    <span class="rsub ellipsis">{jobStatus(j, false)}</span>
                  </div>
                  <span class="jp"><Num value={pct(j)} />%</span>
                  <span class="jbar"><BrushBar value={j.progress} color={j.fg} height={5} /></span>
                </div>
              {/each}
            {/if}
          </section>
        {/if}
      </div>
    {:else if hs === "updating"}
      <section class="jobs" aria-label="Updating now">
        <div class="sec-h"><span>Updating now</span></div>
        {#each store.openJobs as j, i (j.id)}
          <div class="job" animate:flip={{ duration: reduced() ? 0 : 220 }} use:enter={i}>
            <Mark app={j} size={32} />
            <div class="jname">
              <span class="rname ellipsis">{j.name}</span>
              <span class="rsub ellipsis">{j.fromVersion ? `${j.fromVersion}  →  ${j.toVersion}` : j.toVersion}</span>
            </div>
            <span class="jstat ellipsis" style:color={j.state === "paused" ? "var(--warn)" : "var(--text-2)"}>{jobStatus(j)}</span>
            <span class="jpct"><Num value={pct(j)} />%</span>
            <button class="ib" aria-label="{j.state === 'paused' ? 'Resume' : 'Pause'} {j.name}" title={pauseTip(j, "  Space")} onclick={() => pauseJob(j.id, j.state)} disabled={!canPause(j)}>
              <Icon d={j.state === "paused" ? I.play : I.pause} />
            </button>
            <span class="jbar2"><BrushBar value={j.progress} color={j.fg} height={6} fillOpacityOverride={j.state === "paused" ? 0.4 : 0.85} /></span>
          </div>
        {/each}
      </section>
    {:else if hs === "failed"}
      <section class="run" aria-label="This run">
        <div class="sec-h"><span>This run</span></div>
        {#each doneJobs as j, i (j.id)}
          <div class="frow" use:enter={i}>
            <Mark app={j} size={28} />
            <span class="fname ellipsis">{j.name}</span>
            <span class="fres ellipsis" style:color={j.state === "failed" ? "var(--err)" : "var(--ok)"}>
              <Dab color={j.state === "failed" ? "#D9706A" : "#7FB77E"} />
              {j.state === "failed" ? `Failed, ${j.error?.code ?? "error"}${j.error ? ", " + j.error.message.split(". ")[0].toLowerCase() : ""}` : `${j.kind === "install" ? "Installed" : "Updated to"} ${j.toVersion}`}
            </span>
            <span class="fmeta">{j.state === "failed" ? (j.fromVersion ? `${j.fromVersion} kept` : "") : bytes(j.total)}</span>
          </div>
        {/each}
      </section>
    {:else if hs === "repair"}
      <section class="run" aria-label="Needs repair">
        <div class="sec-h"><span>Needs repair</span></div>
        {#each store.missing as a, i (a.id)}
          <div class="frow" use:enter={i}>
            <Mark app={a} size={28} />
            <span class="fname ellipsis">{a.name} {a.version}</span>
            <span class="fres ellipsis" style="color:var(--err)"><Dab color="#D9706A" />Files missing</span>
            {#if canRepair(a)}<Btn variant="raised" size="sm" onclick={() => repairApp(a.id)}>Repair</Btn>{:else}<Btn variant="ghost" size="sm" href={appHref(a.id)}>Details</Btn>{/if}
          </div>
        {/each}
      </section>
    {:else if hs === "current"}
      <div class="grid4">
        {#each store.apps as a, i (a.id)}
          {@const [vm, vb] = splitVersion(a.version)}
          <a class="gtile link" href={appHref(a.id)} use:enter={i}>
            <Mark app={a} size={32} />
            <div class="gcol">
              <span class="gname ellipsis">{a.name}</span>
              <span class="gver">{vm}{#if vb}<span class="vb">{vb}</span>{/if}</span>
              <svg viewBox="0 0 400 24" preserveAspectRatio="none" style="width:56px;height:5px;margin-top:4px"><path d={BRUSH} fill={a.status === "missing" ? "#D9706A" : a.fg} fill-opacity="0.55" /></svg>
            </div>
          </a>
        {/each}
      </div>
    {:else if hs === "ccdown"}
      <section class="req" aria-label="Required services">
        <div class="sec-h"><span>Required for licensing</span></div>
        {#each svc as s, i (s.name)}
          <div class="srow" use:enter={i}>
            <span class="rcol"><span class="rname">{s.displayName}</span><span class="rsub">{s.description}</span></span>
            <span class="fres" style:color={s.running ? "var(--ok)" : "var(--err)"}><Dab color={s.running ? "var(--ok)" : "#D9706A"} />{s.running ? "Running" : "Not running"}</span>
          </div>
        {/each}
        {#if otherUpdates > 0}
          <p class="note">Craft apps, Blender and More apps are not affected. {countOf(otherUpdates, "update")} ready in <a href="#/installed" onclick={review}>Installed</a>.</p>
        {/if}
      </section>
    {:else if hs === "signedout"}
      <p class="foot">Your {countOf(store.apps.filter((a) => a.source === "adobe").length, "Adobe app")} stay on this PC. Checks, installs and updates start again as soon as you are signed in.</p>
    {/if}
  {/if}
</section>

<style>
  .home {
    overflow: hidden;
  }
  .top {
    position: relative;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }
  .lt {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-3);
  }
  .lt.t2 {
    color: var(--text-2);
  }
  .lt.err {
    color: var(--err);
  }
  .chk {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .chk :global(.spin) {
    animation: spin 0.9s linear infinite;
  }
  .selfup {
    position: relative;
    margin-top: 12px;
    display: inline-flex;
    align-items: center;
    align-self: flex-start;
    gap: 10px;
    font-size: 12px;
    color: var(--text-2);
  }
  .selfup .lnk {
    color: var(--text);
    font-weight: 500;
    text-decoration: underline;
    text-decoration-color: var(--text-4);
    text-underline-offset: 3px;
    transition: text-decoration-color var(--t-fast);
  }
  .selfup .lnk:hover {
    text-decoration-color: var(--text);
  }
  .statement-wrap {
    position: relative;
    margin-top: 72px;
  }
  .statement-wrap.wide {
    margin-top: 64px;
    max-width: 760px;
  }
  .statement-wrap.wide :global(h1) {
    line-height: 1.06;
  }
  .lede {
    position: relative;
    margin-top: 24px;
    max-width: 600px;
    font-size: 15px;
    line-height: 1.5;
    color: var(--text-2);
  }
  .actions {
    position: relative;
    display: flex;
    gap: 10px;
    margin-top: 28px;
  }
  .actions:empty {
    display: none;
  }
  .stats.big {
    position: relative;
    margin-top: 36px;
  }
  .statement-wrap:has(+ .stats.big) {
    margin-top: 56px;
  }
  /* ready columns */
  .cols {
    position: relative;
    display: grid;
    grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr);
    gap: 56px;
    margin-top: 60px;
  }
  .cols.single {
    grid-template-columns: minmax(0, 560px);
    margin-top: 60px;
  }
  .ttl {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .row {
    min-height: 56px;
    padding: 8px 0;
    display: flex;
    align-items: center;
    gap: 14px;
    border-top: 1px solid var(--sep);
  }
  .rcol,
  .jname {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .rname {
    font-size: 13px;
    font-weight: 500;
  }
  .rsub {
    font-size: 12px;
    color: var(--text-3);
  }
  .rsize {
    font-size: 13px;
    color: var(--text-2);
    white-space: nowrap;
  }
  .empty {
    padding-top: 18px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-3);
  }
  .jrow {
    position: relative;
    min-height: 56px;
    padding: 8px 0 14px;
    display: flex;
    align-items: center;
    gap: 12px;
    border-top: 1px solid var(--sep);
  }
  .jp {
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .jbar {
    position: absolute;
    left: 40px;
    right: 0;
    bottom: 6px;
  }
  /* updating */
  .jobs {
    position: relative;
    margin-top: 48px;
  }
  .job {
    position: relative;
    height: 64px;
    display: flex;
    align-items: center;
    gap: 14px;
    border-top: 1px solid var(--sep);
  }
  .jname {
    flex: none;
    width: 220px;
  }
  .jstat {
    flex: 1;
    font-size: 12px;
  }
  .jpct {
    width: 64px;
    text-align: right;
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .ib {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    color: var(--text-2);
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background-color var(--t-fast), color var(--t-fast), transform var(--t-fast) var(--ease);
  }
  .ib:hover:not(:disabled) {
    background: var(--raised);
    color: var(--text);
  }
  .ib:active:not(:disabled) {
    transform: scale(0.92);
  }
  .jbar2 {
    position: absolute;
    left: 46px;
    right: 0;
    bottom: 5px;
  }
  /* failed, repair, services */
  .run,
  .req {
    position: relative;
    margin-top: 52px;
    max-width: 640px;
  }
  .req {
    max-width: 560px;
  }
  .frow {
    min-height: 52px;
    display: flex;
    align-items: center;
    gap: 14px;
    border-top: 1px solid var(--sep);
  }
  .fname {
    width: 200px;
    flex: none;
    font-size: 13px;
    font-weight: 500;
  }
  .fres {
    flex: 1;
    min-width: 0;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }
  .fmeta {
    font-size: 12px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .srow {
    min-height: 48px;
    display: flex;
    align-items: center;
    gap: 12px;
    border-top: 1px solid var(--sep);
  }
  .note,
  .foot {
    margin-top: 20px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-3);
  }
  .note a {
    color: var(--text-2);
  }
  .foot {
    position: relative;
    margin-top: 56px;
    max-width: 520px;
  }
  /* app grids */
  .grid4 {
    position: relative;
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    column-gap: 32px;
    row-gap: 40px;
    margin-top: 72px;
  }
  .grid4.first {
    margin-top: 64px;
  }
  .gtile {
    display: flex;
    flex-direction: column;
    gap: 12px;
    color: var(--text);
    min-width: 0;
  }
  .gtile.link {
    transition: transform var(--t-base) var(--ease);
  }
  .gtile.link:hover {
    transform: translateY(-2px);
  }
  .gtile.link:active {
    transform: scale(0.98);
  }
  .gtile.skel {
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .gcol {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .gname {
    font-size: 13px;
    color: var(--text-2);
  }
  .gver {
    font-size: 28px;
    line-height: 1;
    font-weight: 600;
    letter-spacing: var(--track-heading);
    white-space: nowrap;
  }
  .vb {
    font-size: 15px;
    font-weight: 500;
    letter-spacing: 0;
    color: var(--text-3);
  }
  .skel-head {
    display: flex;
    flex-direction: column;
  }
  .skel-btns {
    display: flex;
    gap: 10px;
    margin-top: 32px;
  }
  @media (max-width: 999px) {
    .statement-wrap {
      margin-top: 44px;
    }
    .statement-wrap :global(h1) {
      font-size: 48px !important;
    }
    .cols {
      grid-template-columns: minmax(0, 1fr);
      gap: 28px;
      margin-top: 32px;
    }
    .grid4 {
      grid-template-columns: repeat(3, minmax(0, 1fr));
      margin-top: 44px;
    }
    .jname {
      width: 160px;
    }
  }
</style>
