<script lang="ts">
    let files: FileList | null | undefined = $state();

</script>

<div class="container">
    <input bind:files id="file" type="file" accept="application/epub+zip" />
    <button disabled={!files}>Import</button>
</div>

<style>
    .container {
        height: 100%;
        width: 100%;
        display: flex;
        justify-content: center;
        align-items: center;
    }
</style>