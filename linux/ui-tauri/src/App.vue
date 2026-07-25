<script setup lang="ts">
// Shell: a left icon rail (AppShell) + the routed page (pages/home, settings,
// contacts, …) in the content area. The clipboard-history POPUP window loads
// the same frontend at /#/clipboard — it renders bare (no rail, no nav).
import { computed, onMounted, onUnmounted } from "vue";
import { useRoute } from "vue-router";
import AppShell from "@/components/AppShell.vue";
import OnboardingFlow from "@/pages/onboarding/OnboardingFlow.vue";
import { introDone } from "@/lib/intro";

const route = useRoute();
const bare = computed(() => route.path.startsWith("/clipboard"));

// Suppress the webview's native right-click menu (Reload / Inspect / Back …) —
// a desktop app shouldn't expose it, and an accidental "Reload" there blanked
// the connection state. Components that want a real right-click menu attach
// their own `@contextmenu.prevent` handler (e.g. the device cards' "Forget"):
// those still run, since this only cancels the DEFAULT menu, never their logic.
// Text fields and copyable content (.selectable-text, e.g. SMS bubbles) keep
// the native menu so copy / paste stays available.
function onContextMenu(e: MouseEvent) {
  const el = e.target as HTMLElement | null;
  if (el?.closest('input, textarea, [contenteditable=""], [contenteditable="true"], .selectable-text')) return;
  e.preventDefault();
}
onMounted(() => window.addEventListener("contextmenu", onContextMenu));
onUnmounted(() => window.removeEventListener("contextmenu", onContextMenu));
</script>

<template>
  <!-- The clipboard popup window renders bare; the main window shows the
       first-run intro until it's completed, then the app shell. -->
  <RouterView v-if="bare" />
  <OnboardingFlow v-else-if="!introDone" />
  <AppShell v-else />
</template>
