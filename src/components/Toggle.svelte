<script lang="ts">
  let {
    checked,
    onchange,
    label = "",
    labelledby = "",
    disabled = false,
  }: { checked: boolean; onchange: (v: boolean) => void; label?: string; labelledby?: string; disabled?: boolean } = $props();
</script>

<button class="toggle" class:on={checked} {disabled} role="switch" aria-checked={checked} aria-label={label || undefined} aria-labelledby={labelledby || undefined} onclick={() => onchange(!checked)}>
  <span class="knob"></span>
</button>

<style>
  .toggle {
    position: relative;
    width: 34px;
    height: 20px;
    flex: none;
    border-radius: 10px;
    background: #3a3a3a;
    transition: background-color 0.08s ease;
  }
  .toggle.on {
    background: var(--primary);
  }
  .toggle:disabled {
    opacity: 0.4;
  }
  .knob {
    position: absolute;
    left: 3px;
    top: 3px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--text-2);
    transition: transform 0.09s var(--ease), background-color 0.08s ease;
  }
  .on .knob {
    transform: translateX(14px);
    background: var(--on-primary);
  }
  .toggle:active .knob {
    transform: scaleX(1.15);
  }
  .toggle.on:active .knob {
    transform: translateX(13px) scaleX(1.15);
  }
</style>
