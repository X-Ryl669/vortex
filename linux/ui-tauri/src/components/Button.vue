<script setup lang="ts">
import { computed } from "vue";
import { cn } from "@/lib/utils";

const props = defineProps<{
  variant?: "default" | "outline" | "ghost" | "destructive";
  size?: "default" | "sm" | "lg" | "icon";
  disabled?: boolean;
}>();

const klass = computed(() => {
  const base =
    "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors " +
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background " +
    "disabled:pointer-events-none disabled:opacity-50";
  const variant: Record<string, string> = {
    default: "bg-primary text-primary-foreground hover:bg-primary/90",
    outline:
      "border border-border bg-transparent hover:bg-accent hover:text-accent-foreground",
    ghost: "hover:bg-accent hover:text-accent-foreground",
    destructive: "bg-destructive text-destructive-foreground hover:bg-destructive/90",
  };
  const size: Record<string, string> = {
    default: "h-9 px-4 py-2",
    sm: "h-8 rounded-md px-3 text-xs",
    lg: "h-10 rounded-md px-6",
    icon: "h-9 w-9",
  };
  return cn(base, variant[props.variant ?? "default"], size[props.size ?? "default"]);
});
</script>

<template>
  <button :class="klass" :disabled="disabled">
    <slot />
  </button>
</template>
