<script>
  import "../app.css";
  import { underTitleBar } from "$lib/platform";

  document.documentElement.toggleAttribute("data-under-title-bar", underTitleBar);
  let { children } = $props();
</script>

{@render children()}
