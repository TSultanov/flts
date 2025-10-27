<script lang="ts">
    import { p } from '../router';

    export type RouteLinkProps = {
        href: string;
        label: string;
    };

    let { href, label }: RouteLinkProps = $props();
</script>

<a {href}>{label}</a
>

<style>
    a {
        display: inline-block;
        background-color: var(--background-color);
        color: var(--text-inverted);
        text-decoration: none;
        padding: 10px;
    }

    :global(a.current) {
        background-color: var(--selected-color);
    }

    a:hover {
        background-color: var(--hover-color);
    }
</style>
