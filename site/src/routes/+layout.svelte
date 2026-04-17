<script lang="ts">
  import '../app.css';
  import Nav from '$lib/components/Nav.svelte';
  import Footer from '$lib/components/Footer.svelte';
  let { children } = $props();
</script>

<svelte:head>
  <title>Keplor</title>
</svelte:head>

<Nav />
<main class="pt-14">
  {@render children()}
</main>
<Footer />
