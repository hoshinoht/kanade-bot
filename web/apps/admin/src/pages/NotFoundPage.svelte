<script lang="ts">
  let { path }: { path: string } = $props();
</script>

<!-- v4 error.html, for a path no route matches. -->
<div class="page-head">
  <div>
    <p class="eyebrow">HTTP 404</p>
    <h1>That doesn't exist</h1>
  </div>
</div>
<div class="empty">
  <strong>Nothing lives at <code>{path}</code>.</strong>
  <a href="/">Back to the week</a>
</div>
