<script setup lang="ts">
// The app's standard search field (shared by Contacts, Recents and future
// pages so they keep one look): a soft pill — Search icon + inline input,
// matching the Vortex design system. Forwards Enter for pages that act on
// the typed value (Recents' manual dial).
import { Search } from "lucide-vue-next";

defineProps<{ modelValue: string; placeholder?: string }>();
const emit = defineEmits<{
  (e: "update:modelValue", v: string): void;
  (e: "enter"): void;
}>();
</script>

<template>
  <div
    class="flex items-center gap-2.5 px-3.5 py-2.5 rounded-xl bg-muted/50 border border-border focus-within:border-primary transition-colors"
  >
    <Search class="h-4 w-4 shrink-0 text-muted-foreground" />
    <input
      :value="modelValue"
      type="text"
      data-search
      :placeholder="placeholder"
      class="flex-1 min-w-0 bg-transparent text-[13.5px] outline-none placeholder:text-muted-foreground"
      @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)"
      @keydown.enter="emit('enter')"
    />
  </div>
</template>
