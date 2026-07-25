<script setup lang="ts">
// Shared contact avatar — a soft tinted circle with up to two initials.
// The hue is derived deterministically from the name, so each person keeps
// the same colour across Contacts, Calls and Messages. Palette is the Vortex
// design-system set.
import { computed } from "vue";

const props = withDefaults(defineProps<{ name: string; size?: number }>(), {
  size: 40,
});

const PALETTE = [
  "#5B8DEF",
  "#2ECC71",
  "#E0A65A",
  "#C76B82",
  "#9B7BE0",
  "#3FB6C0",
  "#D77A57",
  "#6BA86B",
];

const initials = computed(() => {
  const words = (props.name || "").trim().split(/\s+/).filter((w) => /[a-z0-9]/i.test(w));
  if (!words.length) return "#";
  return words
    .slice(0, 2)
    .map((w) => w.charAt(0))
    .join("")
    .toUpperCase();
});

const color = computed(() => {
  const key = props.name || "?";
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
  return PALETTE[h % PALETTE.length];
});

const style = computed(() => ({
  width: `${props.size}px`,
  height: `${props.size}px`,
  background: `${color.value}26`, // ~15% tint
  color: color.value,
  fontSize: `${Math.round(props.size * 0.34)}px`,
}));
</script>

<template>
  <span
    class="shrink-0 rounded-full flex items-center justify-center font-semibold leading-none"
    :style="style"
    >{{ initials }}</span
  >
</template>
