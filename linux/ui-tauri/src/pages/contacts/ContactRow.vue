<script setup lang="ts">
// One contact row. Presentation only — the parent owns the data window and
// the message/call actions it emits; the shared Avatar handles the initial +
// deterministic tint so contacts, calls and messages all match.
import { useI18n } from "vue-i18n";
import { Phone, MessageSquare } from "lucide-vue-next";
import type { Contact } from "@/composables/useContacts";
import Avatar from "@/components/Avatar.vue";

const props = defineProps<{ contact: Contact }>();
const emit = defineEmits<{ message: [number: string]; call: [number: string] }>();

const { t } = useI18n();
</script>

<template>
  <div
    class="group h-full w-full flex items-center gap-3 rounded-xl px-3 hover:bg-accent"
  >
    <button
      class="flex items-center gap-3 flex-1 min-w-0 text-left"
      :title="t('messages.title')"
      @click="emit('message', contact.numbers[0])"
    >
      <Avatar :name="contact.name || contact.numbers[0]" :size="40" />
      <span class="flex-1 min-w-0">
        <span class="block text-[14.5px] font-medium truncate">{{ contact.name || contact.numbers[0] }}</span>
        <span class="block text-xs text-muted-foreground truncate mt-0.5">{{ contact.numbers[0] }}</span>
      </span>
    </button>
    <div class="flex items-center gap-2 shrink-0">
      <button
        :title="t('messages.title')"
        class="h-9 w-9 rounded-full flex items-center justify-center bg-muted/60 border border-border text-muted-foreground hover:bg-accent hover:text-foreground transition-colors"
        @click="emit('message', contact.numbers[0])"
      >
        <MessageSquare class="h-4 w-4" />
      </button>
      <button
        :title="t('contacts.call')"
        class="h-9 w-9 rounded-full flex items-center justify-center bg-primary/[0.13] text-primary hover:bg-primary/25 transition-colors"
        @click="emit('call', contact.numbers[0])"
      >
        <Phone class="h-4 w-4" />
      </button>
    </div>
  </div>
</template>
