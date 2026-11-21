<script lang="ts">
  let { size = 24, label }: { size?: number; label?: string } = $props();
  const arms = [0, 60, 120, 180, 240, 300];
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 32 32"
  fill="currentColor"
  role={label ? "img" : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : "true"}
>
  {#each arms as angle (angle)}
    <path transform="rotate({angle} 16 16)" d="M15.25 16 L13.7 7.9 A2.4 2.4 0 1 1 18.3 7.9 L16.75 16 Z" />
  {/each}
  <circle cx="16" cy="16" r="1.9" />
</svg>
