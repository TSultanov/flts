<script lang="ts">
    import SimpleImportView from "./SimpleImportView.svelte";
    import TabGroup from "../widgets/TabGroup.svelte";
    import FileImportView from "./FileImportView.svelte";


</script>

{#snippet simpleImport()}
    <SimpleImportView />
{/snippet}

{#snippet fileImport()}
    <FileImportView />
{/snippet}

<TabGroup tabs={[
    {
        header: "Plain text import",
        content: simpleImport,
    },
    {
        header: "File import",
        content: fileImport
    }
]}/>