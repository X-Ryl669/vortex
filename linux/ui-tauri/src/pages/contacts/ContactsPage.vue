<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { useVirtualList } from "@vueuse/core";
import { Users } from "lucide-vue-next";
import { contacts, contactsLoaded } from "@/composables/useContacts";
import { dial } from "@/lib/dial";
import SearchInput from "@/components/SearchInput.vue";
import ContactRow from "./ContactRow.vue";

const { t } = useI18n();
const router = useRouter();

function message(number: string) {
  // Straight to the thread route — back then pops to THIS page naturally
  // (the old ?to= indirection added a replace hop into history).
  if (number) router.push(`/messages/${encodeURIComponent(number)}`);
}
const query = ref("");

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return contacts.value;
  const qDigits = q.replace(/\s/g, "");
  return contacts.value.filter(
    (c) =>
      c.name.toLowerCase().includes(q) ||
      c.numbers.some((n) => n.replace(/\s/g, "").includes(qDigits)),
  );
});

// Virtualised rendering (same as RecentsPage): only the rows on screen are
// in the DOM, so a 2000-contact phone scrolls as cheaply as a handful.
const ROW_H = 64;
const {
  list: vlist,
  containerProps,
  wrapperProps,
  scrollTo: vScrollTo,
} = useVirtualList(filtered, { itemHeight: ROW_H, overscan: 6 });
watch(query, () => vScrollTo(0));
</script>

<template>
  <div class="h-full flex flex-col">
    <header class="flex items-center gap-2 px-5 py-4 border-b border-border bg-card/30">
      <Users class="h-5 w-5 text-muted-foreground" />
      <h1 class="text-base font-semibold">{{ t("contacts.title") }}</h1>
      <span v-if="contacts.length" class="ml-auto text-xs text-muted-foreground">
        {{ contacts.length }}
      </span>
    </header>

    <!-- Search -->
    <div class="px-4 pt-3 pb-2">
      <SearchInput v-model="query" :placeholder="t('contacts.search')" />
    </div>

    <!-- Virtualised list (only this scrolls; the header + search stay fixed).
         No padding on the scroll container — useVirtualList maps scrollTop
         straight to row offsets, so padding would shift/clip rows. -->
    <main v-bind="containerProps" class="flex-1 min-h-0 overflow-y-auto">
      <div v-bind="wrapperProps">
        <div
          v-for="{ data: c } in vlist"
          :key="c.id"
          class="px-3 py-1"
          :style="{ height: ROW_H + 'px' }"
        >
          <ContactRow :contact="c" @message="message" @call="dial" />
        </div>
      </div>

      <!-- Empty states -->
      <div
        v-if="contactsLoaded && contacts.length === 0"
        class="flex flex-col items-center justify-center text-center text-muted-foreground py-16"
      >
        <Users class="h-10 w-10 mb-3 opacity-40" />
        <p class="text-sm">{{ t("contacts.empty") }}</p>
      </div>
      <div
        v-else-if="query && filtered.length === 0"
        class="text-center text-sm text-muted-foreground py-12"
      >
        {{ t("contacts.no_match") }}
      </div>
    </main>
  </div>
</template>
