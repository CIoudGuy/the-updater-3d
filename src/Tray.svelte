<script lang="ts">
  import { onMount } from "svelte";
  import { flip } from "svelte/animate";
  import Btn from "./components/Btn.svelte";
  import Icon from "./components/Icon.svelte";
  import Mark from "./components/Mark.svelte";
  import BrushBar from "./components/BrushBar.svelte";
  import { BRUSH, SPLASH } from "./lib/paths";
  import { I } from "./lib/icons";
  import { send } from "./lib/api";
  import { store } from "./lib/store.svelte";
  import { updateAll } from "./lib/actions";
  import { bytes, clock, countOf } from "./lib/format";
  import { jobStatus, pct } from "./lib/jobs";
  import { reduced } from "./lib/motion";

  onMount(() => {
    let off = () => {};
    store.init().then((u) => (off = u));
    return () => off();
  });

  const n = $derived(store.updates.length);
  const jobs = $derived(store.openJobs);
  const mood = $derived(!store.status.online ? "#A0A0A0" : n > 0 ? "#D9B26A" : jobs.length ? "#7C93D6" : "#7FB77E");
  const word = $derived(!store.status.online ? "Offline" : n > 0 ? countOf(n, "update") : jobs.length ? "Updating" : "Current");
  const sub = $derived.by(() => {
    if (!store.status.online) return `Last checked at ${clock(store.status.lastCheck)}.`;
    const base = n > 0 && store.updatesBytes > 0 ? `${bytes(store.updatesBytes)}. ` : "";
    return `${base}${store.status.lastCheck ? "Checked at " + clock(store.status.lastCheck) + "." : "Not checked yet."}`;
  });
</script>

<div class="tray">
  <svg class="splash" viewBox="0 0 300 300" aria-hidden="true"><path d={SPLASH[0]} transform="translate(40 30) scale(0.7)" fill={mood} fill-opacity="0.07" /></svg>

  <div class="top">
    <span class="grow"></span>
    <button class="ib" aria-label="Settings" title="Settings  Ctrl ," onclick={() => send("show_main", { route: "settings" })}><Icon d={I.settings} /></button>
  </div>

  <h1>
    <span class="w">
      {#key word + mood}<svg class="under" viewBox="0 0 400 24" preserveAspectRatio="none" aria-hidden="true"><path d={BRUSH} fill={mood} fill-opacity="0.5" /></svg>{/key}
      <span class="t">{word}</span>
    </span>
  </h1>
  <span class="sub">{sub}</span>

  {#if n > 0 && store.status.online}
    <div class="act"><Btn variant="primary" hint="Ctrl U" title="Update all  Ctrl U" onclick={updateAll}>Update all</Btn></div>
  {/if}

  {#if jobs.length}
    <div class="lbl">In progress</div>
    <div class="jl">
      {#each jobs.slice(0, 4) as j (j.id)}
        <div class="job" animate:flip={{ duration: reduced() ? 0 : 200 }}>
          <div class="jt">
            <Mark app={j} size={22} fs={9} />
            <span class="jn ellipsis">{j.name}</span>
            <span class="js">{j.state === "downloading" || j.state === "installing" || j.state === "verifying" ? `${j.state === "downloading" ? "Downloading" : "Installing"}, ${pct(j)}%` : jobStatus(j, false)}</span>
          </div>
          <span class="jb"><BrushBar value={j.progress} color={j.fg} height={5} shine={false} /></span>
        </div>
      {/each}
      {#if jobs.length > 4}<div class="more">and {jobs.length - 4} more</div>{/if}
    </div>
  {/if}

  <div class="grow"></div>
  <div class="foot">
    <Btn variant="ghost" size="md" title="Check now  F5" disabled={store.status.checking} onclick={() => send("check_now", {})}>
      <Icon d={I.refresh} class={store.status.checking ? "spin" : ""} />{store.status.checking ? "Checking" : "Check now"}
    </Btn>
    <span class="grow"></span>
    <Btn variant="raised" size="md" title="Open app  Enter" onclick={() => send("show_main", { route: null })}>Open app <Icon d={I.external} size={14} width={2.5} /></Btn>
    <button class="ib" aria-label="Quit" title="Quit" onclick={() => send("quit_app", {})}><Icon d={I.close} /></button>
  </div>
</div>

<style>
  :global(body.tray) {
    background: var(--bar);
  }
  .tray {
    position: relative;
    width: 100vw;
    height: 100vh;
    display: flex;
    flex-direction: column;
    padding: 18px 20px 16px;
    background: var(--bar);
    overflow: hidden;
  }
  .splash {
    position: absolute;
    right: -80px;
    top: -60px;
    width: 300px;
    height: 300px;
    pointer-events: none;
  }
  .top {
    position: relative;
    height: 28px;
    display: flex;
    align-items: center;
  }
  .grow {
    flex: 1;
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
  .ib:hover {
    background: var(--raised);
    color: var(--text);
  }
  .ib:active {
    transform: scale(0.92);
  }
  h1 {
    position: relative;
    margin: 18px 0 0;
    font-size: 40px;
    line-height: 1;
    font-weight: 600;
    letter-spacing: var(--track-display);
  }
  .w {
    position: relative;
    display: inline-block;
  }
  .t {
    position: relative;
  }
  .under {
    position: absolute;
    left: -6px;
    bottom: -4px;
    width: calc(100% + 14px);
    height: 15px;
    animation: draw 0.42s var(--ease) both;
  }
  .sub {
    position: relative;
    margin-top: 12px;
    font-size: 13px;
    color: var(--text-2);
  }
  .act {
    position: relative;
    margin-top: 18px;
    display: flex;
  }
  .act :global(.btn) {
    flex: 1;
  }
  .lbl {
    position: relative;
    margin-top: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
  }
  .jl {
    position: relative;
  }
  .job {
    height: 52px;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 8px;
    border-top: 1px solid var(--sep);
  }
  .jt {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .jn {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    font-weight: 500;
  }
  .js {
    font-size: 12px;
    color: var(--text-2);
    white-space: nowrap;
  }
  .jb {
    display: block;
    margin-left: 32px;
  }
  .more {
    padding: 8px 0;
    font-size: 12px;
    color: var(--text-3);
    border-top: 1px solid var(--sep);
  }
  .foot {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .foot :global(.spin) {
    animation: spin 0.9s linear infinite;
  }
</style>
