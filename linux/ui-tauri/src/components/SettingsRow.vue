<script setup lang="ts">
import type { Component } from "vue";

// A single grouped settings row — icon tile + title + description on the left,
// an iOS-style switch on the right. Lives INSIDE a section card (unlike the
// standalone ToggleRow card): pass `divider` for every row except the first so
// the rows read as one list. The whole row is clickable.
defineProps<{
  icon: Component;
  title: string;
  desc?: string;
  modelValue: boolean;
  divider?: boolean;
  /// Badge beside the title — "Experimental" for a feature that ships with
  /// requirements the rest of the app does not have.
  tag?: string;
}>();
const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();
</script>

<template>
  <div
    class="flex items-center gap-3.5 px-[18px] py-[15px] cursor-pointer select-none transition-colors hover:bg-foreground/[0.02]"
    :class="divider && 'border-t border-border/60'"
    @click="emit('update:modelValue', !modelValue)"
  >
    <span
      class="h-9 w-9 rounded-[10px] shrink-0 grid place-items-center bg-muted/50 border border-border text-muted-foreground"
    >
      <component :is="icon" class="h-[18px] w-[18px]" :stroke-width="1.8" />
    </span>
    <div class="flex-1 min-w-0">
      <div class="text-sm font-semibold text-foreground leading-tight">
        {{ title }}
        <span v-if="tag" class="vx-tag ml-1.5 inline-block align-[2px]">{{ tag }}</span>
      </div>
      <p v-if="desc" class="text-[12.5px] text-muted-foreground mt-0.5 leading-snug">{{ desc }}</p>
    </div>
    <span
      role="switch"
      :aria-checked="modelValue"
      class="relative h-[27px] w-[46px] shrink-0 rounded-full transition-colors duration-300"
      :class="modelValue ? 'bg-primary' : 'bg-foreground/20'"
    >
      <span
        class="absolute top-[3px] h-[21px] w-[21px] rounded-full bg-white shadow-[0_2px_6px_rgba(0,0,0,0.4)] transition-[left] duration-300"
        :class="modelValue ? 'left-[22px]' : 'left-[3px]'"
      />
    </span>
  </div>
</template>
