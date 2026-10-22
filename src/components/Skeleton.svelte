<script lang="ts">
  import Paint from "./Paint.svelte";

  // Painted placeholder for the current screen, shown while the first snapshot is on its way,
  // so the shell and the page outline are on screen immediately.
  let { name }: { name: string } = $props();

  const TITLES: Record<string, string> = {
    home: "",
    installed: "Installed",
    detail: "",
    get: "Get apps",
    queue: "Queue",
    activity: "Activity",
    services: "Services",
    settings: "Settings",
  };
  const kind = $derived(name === "installed" || name === "get" ? "grid" : name === "settings" ? "settings" : name === "home" ? "home" : "rows");
  const title = $derived(TITLES[name] ?? "");
</script>

<section class="screen tight sk" aria-busy="true" aria-label="Loading">
  {#if kind === "settings"}
    <div class="set">
      <div class="nav">
        <h1 class="h1">{title}</h1>
        {#each Array(6) as _, i (i)}<span class="bar nv" style="animation-delay:{i * 60}ms"></span>{/each}
      </div>
      <div class="rows">
        {#each Array(5) as _, i (i)}<span class="bar row" style="animation-delay:{i * 60}ms"></span>{/each}
      </div>
    </div>
  {:else if kind === "home"}
    <span class="bar big"></span>
    <span class="bar sm"></span>
    <div class="grid g4">
      {#each Array(4) as _, i (i)}<div class="card" style="animation-delay:{i * 70}ms"><Paint kind="blob" fill="#1c1c1c" /></div>{/each}
    </div>
  {:else}
    {#if title}<h1 class="h1">{title}</h1>{:else}<span class="bar big"></span>{/if}
    {#if kind === "grid"}
      <div class="grid g4">
        {#each Array(8) as _, i (i)}<div class="card" style="animation-delay:{i * 60}ms"><Paint kind="blob" fill="#1c1c1c" /></div>{/each}
      </div>
    {:else}
      <div class="rows lone">
        {#each Array(5) as _, i (i)}<span class="bar row" style="animation-delay:{i * 60}ms"></span>{/each}
      </div>
    {/if}
  {/if}
</section>

<style>
  .sk {
    gap: 0;
  }
  .bar {
    display: block;
    border-radius: 6px;
    background: #1c1c1c;
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .big {
    width: 260px;
    height: 40px;
    margin-bottom: 14px;
  }
  .sm {
    width: 380px;
    height: 14px;
  }
  .grid {
    display: grid;
    gap: 16px;
    margin-top: 32px;
  }
  .g4 {
    grid-template-columns: repeat(4, minmax(0, 1fr));
  }
  .card {
    position: relative;
    height: 150px;
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .set {
    display: flex;
    gap: 56px;
  }
  .nav {
    width: 168px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .nav .h1 {
    margin-bottom: 16px;
  }
  .nv {
    height: 26px;
  }
  .rows {
    flex: 1;
    max-width: 760px;
    padding-top: 52px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .rows.lone {
    padding-top: 32px;
  }
  .row {
    height: 44px;
  }
  @media (max-width: 999px) {
    .g4 {
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }
    .set {
      gap: 32px;
    }
    .nav {
      width: 130px;
    }
  }
</style>
