<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  let { path }: { path: string } = $props();
</script>

<!-- v4 error.html, for a path no route matches. -->
<PageLine>
  <h1>That doesn't exist</h1>
  <p class="pageline__context">HTTP 404</p>
</PageLine>
<div class="empty">
  <strong>Nothing lives at <code>{path}</code>.</strong>
  <a href="/">Back to the week</a>
</div>
