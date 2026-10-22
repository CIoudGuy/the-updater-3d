<script lang="ts">
  import { flip } from "svelte/animate";
  import Paint from "./Paint.svelte";
  import Dab from "./Dab.svelte";
  import Icon from "./Icon.svelte";
  import Btn from "./Btn.svelte";
  import { I } from "../lib/icons";
  import { store } from "../lib/store.svelte";
  import { send } from "../lib/api";
  import { router, appHref } from "../lib/router.svelte";
  import { slideIn, reduced } from "../lib/motion";

  const COLORS = { ok: "var(--ok)", warn: "var(--warn)", err: "var(--err)", info: "var(--text-3)" } as const;

  function failedJobFor(appId?: string) {
    return appId ? store.failedJobs.find((j) => j.appId === appId) : undefined;
  }
  function open(appId?: string, id?: number) {
    if (appId && store.app(appId)) router.go(appHref(appId));
    if (id !== undefined) store.dismissToast(id);
  }
</script>

<div class="stack" aria-live="polite">
  {#each store.toasts as t (t.id)}
    {@const fj = t.kind === "err" ? failedJobFor(t.appId) : undefined}
    <div class="toast" role="status" animate:flip={{ duration: reduced() ? 0 : 200 }} in:slideIn|global out:slideIn|global={{ duration: 160, x: 18 }}>
      <Paint kind="blob" fill="#262626" />
      <div class="row">
        <Dab color={COLORS[t.kind]} />
        <button class="text" onclick={() => open(t.appId, t.id)} disabled={!t.appId || !store.app(t.appId)}>{t.text}</button>
        {#if fj}
          <Btn variant="raised" size="sm" title="Retry  R" onclick={() => { send("retry_job", { jobId: fj.id }); store.dismissToast(t.id); }}>Retry</Btn>
        {/if}
        <button class="x" aria-label="Dismiss" title="Dismiss  Esc" onclick={() => store.dismissToast(t.id)}><Icon d={I.close} size={14} width={2.5} /></button>
      </div>
    </div>
  {/each}
</div>

<style>
  .stack {
    position: fixed;
    right: 24px;
    bottom: 24px;
    z-index: 90;
    width: 360px;
    max-width: calc(100vw - 32px);
    display: flex;
    flex-direction: column;
    gap: 8px;
    pointer-events: none;
  }
  .toast {
    position: relative;
    padding: 12px 12px 12px 16px;
    pointer-events: auto;
  }
  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .text {
    flex: 1;
    min-width: 0;
    text-align: left;
    font-size: 13px;
    line-height: 1.4;
    color: var(--text);
    user-select: text;
  }
  .text:disabled {
    color: var(--text);
  }
  .x {
    width: 28px;
    height: 28px;
    flex: none;
    border-radius: 6px;
    color: var(--text-3);
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background-color var(--t-fast), color var(--t-fast);
  }
  .x:hover {
    background: var(--raised-2);
    color: var(--text);
  }
</style>
