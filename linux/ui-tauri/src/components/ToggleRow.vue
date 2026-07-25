<script setup lang="ts">
import type { Component } from "vue";

// A compact settings row: icon + title + hint on the left act as the label,
// an iOS-style switch on the right. The whole card is clickable so the row
// itself toggles the value. Keeps the settings list short as it grows.
defineProps<{
  icon: Component;
  title: string;
  hint?: string;
  modelValue: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();
</script>

<template>
  <section
    class="rounded-xl border border-border bg-card px-4 py-3 flex items-center gap-3 cursor-pointer select-none hover:bg-accent/30 transition-colors"
    @click="emit('update:modelValue', !modelValue)"
  >
    <component :is="icon" class="h-5 w-5 text-muted-foreground shrink-0" />
    <div class="flex-1 min-w-0">
      <div class="text-sm font-medium leading-tight">{{ title }}</div>
      <p v-if="hint" class="text-xs text-muted-foreground mt-0.5">{{ hint }}</p>
    </div>
    <span
      role="switch"
      :aria-checked="modelValue"
      class="relative h-6 w-11 shrink-0 rounded-full transition-colors"
      :class="modelValue ? 'bg-emerald-500' : 'bg-foreground/25'"
    >
      <span
        class="absolute top-0.5 left-0.5 h-5 w-5 rounded-full bg-white shadow-sm transition-transform"
        :class="modelValue ? 'translate-x-5' : 'translate-x-0'"
      />
    </span>
  </section>
</template>
