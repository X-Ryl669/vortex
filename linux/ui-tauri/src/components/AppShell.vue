<script setup lang="ts">
// App shell: the left Sidebar + the routed page in the content area. The shell
// owns only cross-page input behaviours; section switching lives in Sidebar.
import { onMounted, onUnmounted, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import Sidebar from "@/components/Sidebar.vue";
import { primaryPeer } from "@/composables/useHome";

const route = useRoute();
const router = useRouter();

// The sidebar's sections (contacts, recents, messages, notes) are all phone
// features — meaningless until a phone has been paired. So on first launch (no
// device in the store) the nav is hidden and the Devices page stands alone
// full-width to pair from; the sidebar appears once a peer exists. If the last
// device is forgotten while we're on a sub-page, fall back to Home so we're
// never stranded without nav.
watch(primaryPeer, (peer) => {
  if (!peer && route.path !== "/") router.replace("/");
});

// Mouse back/forward buttons (3/4) drive router history — the webview doesn't
// map them itself, and with the open chat living in the URL this makes "mouse
// back closes the chat" work like in a browser.
function onMouseNav(e: MouseEvent) {
  if (e.button === 3) {
    e.preventDefault();
    router.back();
  } else if (e.button === 4) {
    e.preventDefault();
    router.forward();
  }
}
// Desktop keyboard: Esc closes an open chat (pops history); Ctrl+F focuses the
// current page's search field (SearchInput marks its input).
function onKeyNav(e: KeyboardEvent) {
  if (e.key === "Escape" && /^\/messages\/./.test(route.path)) {
    router.back();
  } else if ((e.ctrlKey || e.metaKey) && e.key === "f") {
    const el = document.querySelector<HTMLInputElement>("input[data-search]");
    if (el) {
      e.preventDefault();
      el.focus();
      el.select();
    }
  }
}
onMounted(() => {
  window.addEventListener("mouseup", onMouseNav);
  window.addEventListener("keydown", onKeyNav);
});
onUnmounted(() => {
  window.removeEventListener("mouseup", onMouseNav);
  window.removeEventListener("keydown", onKeyNav);
});
</script>

<template>
  <div class="flex h-screen overflow-hidden bg-background text-foreground">
    <Sidebar v-if="primaryPeer" />

    <!-- Content — a faint emerald glow bleeds in from the top-right corner. -->
    <main
      class="flex-1 min-w-0 overflow-y-auto scrollbar-thin"
      style="background: radial-gradient(120% 80% at 100% -5%, hsl(var(--primary) / 0.05), transparent 50%)"
    >
      <router-view />
    </main>
  </div>
</template>
