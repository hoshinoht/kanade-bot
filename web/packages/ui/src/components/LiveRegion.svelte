<script lang="ts">
  let { message, assertive = false }: { message: string; assertive?: boolean } = $props();
</script>

<div class="vh" role={assertive ? 'alert' : 'status'} aria-live={assertive ? 'assertive' : 'polite'} aria-atomic="true">
  {message}
</div>
