<script lang="ts">
  import { onMount } from "svelte";
  import Btn from "../components/Btn.svelte";
  import Icon from "../components/Icon.svelte";
  import Dab from "../components/Dab.svelte";
  import Paint from "../components/Paint.svelte";
  import Num from "../components/Num.svelte";
  import { BRUSH } from "../lib/paths";
  import { I } from "../lib/icons";
  import { call, send } from "../lib/api";
  import { enter } from "../lib/motion";
  import type { ServiceInfo } from "../lib/types";

  let list = $state<ServiceInfo[] | null>(null);
  let busy = $state<string | null>(null);

  async function refresh() {
    list = await call("get_services", {});
  }
  onMount(() => {
    refresh();
    const t = setInterval(refresh, 2000);
    return () => clearInterval(t);
  });

  const running = $derived((list ?? []).filter((s) => s.running));
  const cpu = $derived(running.reduce((n, s) => n + s.cpu, 0));
  const mem = $derived(running.reduce((n, s) => n + s.memory, 0));
  const safeMem = $derived(running.filter((s) => s.safeToStop).reduce((n, s) => n + s.memory, 0));
  const maxMem = $derived(Math.max(1, ...(list ?? []).map((s) => s.memory)));
  const anySafe = $derived(running.some((s) => s.safeToStop && !s.required));

  async function act(cmd: "stop_service" | "start_service", name: string) {
    busy = name;
    await send(cmd, { name });
    await refresh();
    busy = null;
  }
  async function stopSafe() {
    busy = "*";
    await send("stop_safe_services", {});
    await refresh();
    busy = null;
  }
  const mb = (n: number) => Math.round(n / 1e6);
</script>

<section class="screen tight svc">
  <div class="head">
    <div class="tt">
      <h1 class="h1">Background services</h1>
      <span class="desc">Adobe processes running on this PC. A stopped service frees its memory until Creative Cloud starts it again.</span>
    </div>
    <Btn variant="secondary" size="md" title="Stop all safe services  Ctrl Shift S" disabled={!anySafe || busy === "*"} onclick={stopSafe}>Stop all safe services</Btn>
  </div>

  <div class="stats">
    <div class="stat"><span class="k">Running</span><span class="v"><Num value={running.length} /> <span class="u">of {list?.length ?? 0}</span></span></div>
    <div class="stat"><span class="k">CPU</span><span class="v"><Num value={cpu} fmt={(n) => n.toFixed(1)} />%</span></div>
    <div class="stat"><span class="k">Memory</span><span class="v"><Num value={mb(mem)} /> <span class="u">MB</span></span></div>
    <div class="stat"><span class="k">Safe to stop</span><span class="v"><Num value={mb(safeMem)} /> <span class="u">MB</span></span></div>
  </div>

  <div class="table" role="table" aria-label="Services">
    <div class="th" role="row">
      <span role="columnheader">Process</span><span role="columnheader">CPU</span><span role="columnheader">Memory</span><span role="columnheader" class="r">Status</span>
    </div>
    {#if !list}
      {#each [0, 1, 2, 3] as i (i)}<div class="tr skel" aria-hidden="true" style="animation-delay:{i * 100}ms"></div>{/each}
    {:else}
      {#each list as s, i (s.name)}
        <div class="tr" role="row" use:enter={{ i, y: 6 }}>
          <span role="cell" class="pc">
            <span class="nm" style:color={s.running ? "var(--text)" : "var(--text-3)"}>{s.displayName}</span>
            <span class="ds ellipsis" title={s.description}>{s.description}</span>
          </span>
          <span role="cell" class="cp">{s.running ? s.cpu.toFixed(1) + "%" : ""}</span>
          <span role="cell" class="me">
            {#if s.running}
              <span class="mv"><Num value={mb(s.memory)} /> MB</span>
              <span class="mbar" style="transform: scaleX({Math.max(0.06, s.memory / maxMem)})">
                <svg viewBox="0 0 400 24" preserveAspectRatio="none" aria-hidden="true"><path d={BRUSH} fill="#A0A0A0" fill-opacity="0.45" /></svg>
              </span>
            {/if}
          </span>
          <span role="cell" class="stt">
            {#if s.required}
              <span class="badge"><Paint kind="btn" fill="#D9B26A" opacity={0.12} /><Icon d={I.lock} size={13} width={2.5} class="lock" /><span class="bt">Required for licensing</span></span>
            {/if}
            {#if s.running && !s.required}
              <span class="ok"><Dab color="#7FB77E" size={7} />Running</span>
              <Btn variant="raised" size="sm" title="Stop {s.displayName}" disabled={busy === s.name || !s.safeToStop} onclick={() => act("stop_service", s.name)}>Stop</Btn>
            {:else if !s.running}
              <span class="off">{s.required ? "Not running" : "Stopped"}</span>
              <Btn variant={s.required ? "raised" : "ghost"} size="sm" title="Start {s.displayName}" disabled={busy === s.name} onclick={() => act("start_service", s.name)}>Start</Btn>
            {/if}
          </span>
        </div>
      {/each}
    {/if}
  </div>
</section>

<style>
  .head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 24px;
  }
  .tt {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .desc {
    max-width: 620px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-3);
  }
  .stats {
    margin-top: 28px;
  }
  .table {
    margin-top: 28px;
  }
  .th,
  .tr {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 72px 170px 230px;
    align-items: center;
  }
  .th {
    height: 28px;
    font-size: 12px;
    color: var(--text-3);
  }
  .th .r {
    text-align: right;
  }
  .tr {
    height: 46px;
    border-top: 1px solid var(--sep);
    font-size: 13px;
  }
  .tr.skel {
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .pc {
    display: flex;
    align-items: baseline;
    gap: 12px;
    min-width: 0;
  }
  .nm {
    font-weight: 500;
    white-space: nowrap;
    transition: color 0.2s;
  }
  .ds {
    font-size: 12px;
    color: var(--text-3);
  }
  .cp {
    color: var(--text-2);
  }
  .me {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-2);
  }
  .mv {
    width: 58px;
  }
  .mbar {
    display: block;
    width: 64px;
    height: 5px;
    transform-origin: 0 50%;
    transition: transform 0.6s var(--ease);
  }
  .mbar svg {
    width: 100%;
    height: 5px;
  }
  .stt {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 10px;
  }
  .badge {
    position: relative;
    height: 26px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px;
    font-size: 12px;
    color: var(--text-2);
  }
  .badge :global(.lock) {
    position: relative;
    color: var(--ochre);
  }
  .bt {
    position: relative;
  }
  .ok {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--ok);
  }
  .off {
    font-size: 12px;
    color: var(--text-3);
  }
  @media (max-width: 999px) {
    .th,
    .tr {
      grid-template-columns: minmax(0, 1fr) 60px 150px 210px;
    }
    .ds {
      display: none;
    }
  }
</style>
