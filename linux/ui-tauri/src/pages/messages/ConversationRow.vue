<script setup lang="ts">
// One SMS conversation row (the Messages list). Presentation only — the
// avatar initial + relative-time formatting live here; the parent owns the
// data window and handles the `open` it emits. The display name + unread
// state are already resolved into the `conv` it receives.
import { useI18n } from "vue-i18n";
import type { SmsMessage } from "@/composables/useMessages";
import Avatar from "@/components/Avatar.vue";

export interface Conversation {
  address: string;
  name: string;
  last: SmsMessage;
  count: number;
  unread: boolean;
  unreadCount: number;
}

defineProps<{ conv: Conversation }>();
const emit = defineEmits<{ open: [address: string] }>();

const { t } = useI18n();

function relTime(ms: number): string {
  if (!ms) return "";
  const d = Date.now() - ms;
  const min = Math.floor(d / 60000);
  if (min < 1) return t("recents.now");
  if (min < 60) return `${min}m`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h}h`;
  const days = Math.floor(h / 24);
  if (days === 1) return t("recents.yesterday");
  if (days < 7) return `${days}d`;
  return new Date(ms).toLocaleDateString();
}
</script>

<template>
  <button
    class="h-full w-full flex items-center gap-3 rounded-lg px-3 text-left hover:bg-accent"
    @click="emit('open', conv.address)"
  >
    <Avatar :name="conv.name" :size="40" />
    <span class="flex-1 min-w-0">
      <span class="block text-sm truncate" :class="conv.unread ? 'font-bold' : 'font-medium'">{{ conv.name }}</span>
      <span
        class="block text-xs truncate"
        :class="conv.unread ? 'font-semibold text-foreground' : 'text-muted-foreground'"
      >
        <span v-if="conv.last.type === 2" class="opacity-70 font-normal">{{ t("messages.you") }}: </span>{{ conv.last.body }}
      </span>
    </span>
    <span class="flex flex-col items-end gap-1 shrink-0">
      <span class="text-xs" :class="conv.unread ? 'text-emerald-500 font-medium' : 'text-muted-foreground'">{{ relTime(conv.last.date) }}</span>
      <span
        v-if="conv.unread"
        class="min-w-[18px] h-[18px] px-1 rounded-full bg-emerald-500 text-white text-[10px] font-semibold flex items-center justify-center"
      >
        {{ conv.unreadCount > 99 ? "99+" : conv.unreadCount || 1 }}
      </span>
    </span>
  </button>
</template>
