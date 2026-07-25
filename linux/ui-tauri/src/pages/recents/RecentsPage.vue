<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { useVirtualList } from "@vueuse/core";
import { Phone } from "lucide-vue-next";
import { allCalls, callLogLoaded } from "@/composables/useRecents";
import { dial } from "@/lib/dial";
import SearchInput from "@/components/SearchInput.vue";
import RecentRow from "./RecentRow.vue";

const { t } = useI18n();
const router = useRouter();

// One input, two jobs: filters the history below AND — when it looks like a
// phone number — offers a direct "Call <number>" action (manual dial).
const query = ref("");

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return allCalls.value;
  const qDigits = q.replace(/\D/g, "");
  return allCalls.value.filter(
    (c) =>
      c.name.toLowerCase().includes(q) ||
      (qDigits.length > 0 && c.number.replace(/\D/g, "").includes(qDigits)),
  );
});

/** The trimmed query when it's a dialable number (digits with optional
 *  leading + and separators), else null. */
const dialable = computed(() => {
  const q = query.value.trim();
  if (!/^\+?[\d\s()-]{3,}$/.test(q)) return null;
  const n = q.replace(/[\s()-]/g, "");
  return /\d{3,}/.test(n) ? n : null;
});

function callTyped() {
  const n = dialable.value;
  if (n) {
    dial(n);
    query.value = "";
  }
}

// Virtualised rendering: the full history is ~1000+ rows (and grows). Only
// the rows on screen (plus a little overscan) are ever in the DOM — fixed
// row height makes a 10k-row list scroll as cheaply as a 10-row one.
const ROW_H = 64;
const {
  list: vlist,
  containerProps,
  wrapperProps,
  scrollTo: vScrollTo,
} = useVirtualList(filtered, { itemHeight: ROW_H, overscan: 6 });
// Filtering changes the dataset — jump back to the top so the viewport
// isn't left scrolled past the (now shorter) result set.
watch(query, () => vScrollTo(0));

function message(number: string) {
  // Straight to the thread route — back then pops to THIS page naturally.
  if (number) router.push(`/messages/${encodeURIComponent(number)}`);
}
</script>

<template>
  <div class="h-full flex flex-col">
    <header class="flex items-center gap-2 px-5 py-4 border-b border-border bg-card/30">
      <Phone class="h-5 w-5 text-muted-foreground" />
      <h1 class="text-base font-semibold">{{ t("recents.title") }}</h1>
      <span v-if="allCalls.length" class="ml-auto text-xs text-muted-foreground">
        {{ allCalls.length }}
      </span>
    </header>

    <!-- Search / manual dial (same block layout as ContactsPage) -->
    <div class="px-4 pt-3 pb-2">
      <SearchInput v-model="query" :placeholder="t('recents.search')" @enter="callTyped" />
    </div>

    <!-- Manual dial: the typed query is a valid number → call it directly.
         OUTSIDE the virtual scroll area so it never offsets the row math. -->
    <button
      v-if="dialable"
      class="mx-3 mb-1 flex items-center gap-3 rounded-lg pl-3 pr-2 py-2 bg-emerald-500/10 hover:bg-emerald-500/20 transition-colors"
      @click="callTyped"
    >
      <Phone class="h-4 w-4 shrink-0 text-emerald-500" />
      <span class="flex-1 text-left text-sm font-medium text-emerald-600 dark:text-emerald-400 truncate">
        {{ t("recents.callNumber", { n: dialable }) }}
      </span>
    </button>

    <!-- Virtualised list (only this scrolls; the header stays fixed). No
         padding on the scroll container — useVirtualList maps scrollTop
         straight to row offsets, so padding would shift/clip rows. -->
    <main v-bind="containerProps" class="flex-1 min-h-0 overflow-y-auto">
      <div v-bind="wrapperProps">
        <div
          v-for="{ data: c } in vlist"
          :key="c.id"
          class="px-3 py-1"
          :style="{ height: ROW_H + 'px' }"
        >
          <RecentRow :call="c" @message="message" @call="dial" />
        </div>
      </div>

      <div
        v-if="callLogLoaded && filtered.length === 0 && !dialable"
        class="flex flex-col items-center justify-center text-center text-muted-foreground py-16"
      >
        <Phone class="h-10 w-10 mb-3 opacity-40" />
        <p class="text-sm">{{ query ? t("recents.noResults") : t("recents.empty") }}</p>
      </div>
    </main>
  </div>
</template>
