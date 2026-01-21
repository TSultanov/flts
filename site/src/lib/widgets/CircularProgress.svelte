<script lang="ts">
    let {
        value,
        max,
        size = "1em",
        strokeWidth = 3,
        color = "var(--text-inverted)",
    }: {
        value: number;
        max: number;
        size?: string;
        strokeWidth?: number;
        color?: string;
    } = $props();

    const radius = 10;
    const circumference = 2 * Math.PI * radius;

    const percent = $derived(
        Math.max(0, Math.min(1, max > 0 ? value / max : 0)),
    );
    const dashOffset = $derived(circumference * (1 - percent));
</script>

<div class="circular-progress" style:width={size} style:height={size}>
    <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke={color}
        stroke-width={strokeWidth}
        stroke-linecap="round"
    >
        <circle cx="12" cy="12" r={radius} stroke-opacity="0.2" />

        <circle
            cx="12"
            cy="12"
            r={radius}
            stroke-dasharray={circumference}
            stroke-dashoffset={dashOffset}
            transform="rotate(-90 12 12)"
        />
    </svg>
</div>

<style>
    .circular-progress {
        display: inline-block;
        vertical-align: middle;
        position: relative;
    }

    svg {
        display: block;
        width: 100%;
        height: 100%;
        overflow: visible;
    }

    circle {
        transition: stroke-dashoffset 0.1s linear;
    }
</style>
