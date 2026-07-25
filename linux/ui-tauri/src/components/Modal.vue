<script setup lang="ts">
defineProps<{ open: boolean; title?: string }>();
defineEmits<{ (e: "dismiss"): void }>();
</script>

<template>
  <Transition
    enter-active-class="transition-opacity duration-150"
    leave-active-class="transition-opacity duration-100"
    enter-from-class="opacity-0"
    leave-to-class="opacity-0"
  >
    <div
      v-if="open"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-6"
      @click.self="$emit('dismiss')"
    >
      <div class="w-full max-w-md rounded-xl border border-border bg-card p-6 shadow-2xl">
        <h2 v-if="title" class="text-lg font-semibold mb-4">{{ title }}</h2>
        <slot />
      </div>
    </div>
  </Transition>
</template>
