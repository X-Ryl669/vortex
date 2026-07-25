<script setup lang="ts">
const props = defineProps<{ title?: string; subtitle?: string }>();
</script>

<template>
  <div class="rounded-xl border border-border bg-card text-card-foreground shadow-sm">
    <div v-if="props.title || props.subtitle || $slots.header" class="flex flex-col gap-1.5 p-6 pb-3">
      <slot name="header">
        <h3 v-if="props.title" class="text-base font-semibold leading-none">{{ props.title }}</h3>
        <p v-if="props.subtitle" class="text-sm text-muted-foreground">{{ props.subtitle }}</p>
      </slot>
    </div>
    <div class="p-6 pt-3">
      <slot />
    </div>
  </div>
</template>
