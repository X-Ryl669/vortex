<script setup lang="ts">
import { computed } from "vue";

const props = defineProps<{ rssi: number }>();

// 4 bars based on RSSI. Stronger (less negative) = more bars.
//   ≥ -55  → 4 bars  (very strong)
//   ≥ -65  → 3 bars
//   ≥ -75  → 2 bars
//   ≥ -85  → 1 bar
//   else   → 1 bar (weak)
const bars = computed(() => {
  const r = props.rssi;
  if (r >= -55) return 4;
  if (r >= -65) return 3;
  if (r >= -75) return 2;
  return 1;
});
</script>

<template>
  <div class="flex items-end gap-0.5 h-4">
    <div
      v-for="i in 4"
      :key="i"
      class="w-1 rounded-sm transition-colors"
      :class="i <= bars ? 'bg-primary' : 'bg-muted'"
      :style="{ height: `${(i / 4) * 100}%` }"
    />
  </div>
</template>
