<script setup lang="ts">
// One call-history row (Recents). Presentation only — the type→icon/colour,
// relative-date and duration formatting live here; the parent owns the data
// window and handles the message/call actions it emits.
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { ArrowDownLeft, ArrowUpRight, MessageSquare, Phone } from "lucide-vue-next";
import type { CallLogEntry } from "@/composables/useRecents";
import Avatar from "@/components/Avatar.vue";

const props = defineProps<{ call: CallLogEntry }>();
const emit = defineEmits<{ message: [number: string]; call: [number: string] }>();

const { t } = useI18n();

const displayName = computed(
  () => props.call.name || props.call.number || t("recents.unknown"),
);

// CallLog.Calls.TYPE → arrow + colour + label.
const meta = computed(() => {
  switch (props.call.type) {
    case 2:
      return { icon: ArrowUpRight, color: "text-sky-500", label: t("recents.outgoing") };
    case 3:
      return { icon: ArrowDownLeft, color: "text-rose-500", label: t("recents.missed") };
    case 5:
      return { icon: ArrowDownLeft, color: "text-amber-500", label: t("recents.rejected") };
    default:
      return { icon: ArrowDownLeft, color: "text-emerald-500", label: t("recents.incoming") };
  }
});

// "Today 14:20" for today, "Yesterday", a weekday within a week, else a date.
function dateLabel(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  const now = new Date();
  const day = (x: Date) => x.toDateString();
  if (day(d) === day(now)) {
    const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
    return `${t("recents.today")} ${time}`;
  }
  const yest = new Date(now);
  yest.setDate(now.getDate() - 1);
  if (day(d) === day(yest)) return t("recents.yesterday");
  if (now.getTime() - d.getTime() < 7 * 86400000) {
    return d.toLocaleDateString([], { weekday: "short" });
  }
  return d.toLocaleDateString();
}

// Call length as m:ss; blank when there's nothing to show (missed / no answer).
function duration(sec: number): string {
  if (!sec || sec < 1) return "";
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}
</script>

<template>
  <div
    class="group h-full w-full flex items-center gap-3 rounded-xl px-3 hover:bg-accent"
  >
    <Avatar :name="displayName" :size="40" />
    <span class="flex-1 min-w-0">
      <span class="block text-[14.5px] font-medium truncate">{{ displayName }}</span>
      <span class="flex items-center gap-1.5 mt-0.5 text-xs min-w-0">
        <component :is="meta.icon" class="h-3.5 w-3.5 shrink-0" :class="meta.color" />
        <span class="font-medium shrink-0" :class="meta.color">{{ meta.label }}</span>
        <span class="text-muted-foreground truncate">· {{ dateLabel(call.date) }}</span>
      </span>
    </span>
    <span class="text-xs text-muted-foreground shrink-0 tabular-nums">{{ duration(call.duration) }}</span>
    <div v-if="call.number" class="flex items-center gap-2 shrink-0">
      <button
        :title="t('messages.title')"
        class="h-9 w-9 rounded-full flex items-center justify-center bg-muted/60 border border-border text-muted-foreground hover:bg-accent hover:text-foreground transition-colors"
        @click="emit('message', call.number)"
      >
        <MessageSquare class="h-4 w-4" />
      </button>
      <button
        :title="t('contacts.call')"
        class="h-9 w-9 rounded-full flex items-center justify-center bg-primary/[0.13] text-primary hover:bg-primary/25 transition-colors"
        @click="emit('call', call.number)"
      >
        <Phone class="h-4 w-4" />
      </button>
    </div>
  </div>
</template>
