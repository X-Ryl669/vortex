<script setup lang="ts">
import { ref, computed, watch } from "vue";
import { PopoverRoot, PopoverTrigger, PopoverPortal, PopoverContent } from "radix-vue";
import { useI18n } from "vue-i18n";
import { Bell, ChevronLeft, ChevronRight } from "lucide-vue-next";

const props = defineProps<{ modelValue: number }>();
const emit = defineEmits<{ "update:modelValue": [number] }>();

const { t } = useI18n();
const open = ref(false);

const init = () => (props.modelValue > 0 ? new Date(props.modelValue) : new Date());
const viewYear = ref(init().getFullYear());
const viewMonth = ref(init().getMonth());
const sel = ref<{ y: number; m: number; d: number } | null>(
  props.modelValue > 0 ? { y: init().getFullYear(), m: init().getMonth(), d: init().getDate() } : null,
);
const hour = ref(props.modelValue > 0 ? init().getHours() : 9);
const minute = ref(props.modelValue > 0 ? init().getMinutes() : 0);

// Re-seed when the popover opens (the bound value may have changed elsewhere).
watch(open, (o) => {
  if (!o) return;
  const d = init();
  viewYear.value = d.getFullYear();
  viewMonth.value = d.getMonth();
  sel.value = props.modelValue > 0 ? { y: d.getFullYear(), m: d.getMonth(), d: d.getDate() } : null;
  hour.value = props.modelValue > 0 ? d.getHours() : 9;
  minute.value = props.modelValue > 0 ? d.getMinutes() : 0;
});

const dows = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
const monthLabel = computed(() =>
  new Date(viewYear.value, viewMonth.value, 1).toLocaleDateString([], { month: "long" }),
);
const cells = computed<(number | null)[]>(() => {
  const startDow = new Date(viewYear.value, viewMonth.value, 1).getDay();
  const days = new Date(viewYear.value, viewMonth.value + 1, 0).getDate();
  const out: (number | null)[] = [];
  for (let i = 0; i < startDow; i++) out.push(null);
  for (let d = 1; d <= days; d++) out.push(d);
  while (out.length % 7 !== 0) out.push(null);
  return out;
});

const today = new Date();
const isToday = (d: number) =>
  today.getFullYear() === viewYear.value && today.getMonth() === viewMonth.value && today.getDate() === d;
const isSelected = (d: number) =>
  !!sel.value && sel.value.y === viewYear.value && sel.value.m === viewMonth.value && sel.value.d === d;

function prevMonth() {
  if (viewMonth.value === 0) { viewMonth.value = 11; viewYear.value--; } else viewMonth.value--;
}
function nextMonth() {
  if (viewMonth.value === 11) { viewMonth.value = 0; viewYear.value++; } else viewMonth.value++;
}
function pick(d: number) {
  sel.value = { y: viewYear.value, m: viewMonth.value, d };
  apply();
}
function apply() {
  if (!sel.value) sel.value = { y: today.getFullYear(), m: today.getMonth(), d: today.getDate() };
  hour.value = Math.min(23, Math.max(0, Math.floor(hour.value || 0)));
  minute.value = Math.min(59, Math.max(0, Math.floor(minute.value || 0)));
  emit("update:modelValue", new Date(sel.value.y, sel.value.m, sel.value.d, hour.value, minute.value, 0, 0).getTime());
}
function clear() {
  emit("update:modelValue", 0);
}

// Trigger label: "Jun 14 · 2:36 PM" (year only when not the current one).
const label = computed(() => {
  if (!props.modelValue) return t("notes.add_reminder");
  const d = new Date(props.modelValue);
  const opts: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" };
  if (d.getFullYear() !== new Date().getFullYear()) opts.year = "numeric";
  return `${d.toLocaleDateString([], opts)} · ${d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}`;
});
const pad = (n: number) => String(n).padStart(2, "0");
</script>

<template>
  <PopoverRoot v-model:open="open">
      <PopoverTrigger
        class="rounded-md p-1.5 transition-colors hover:bg-secondary"
        :class="modelValue > 0 ? 'text-primary' : 'text-muted-foreground'"
        :title="modelValue > 0 ? label : t('notes.add_reminder')"
      >
        <Bell class="h-4 w-4" />
      </PopoverTrigger>
      <PopoverPortal>
        <PopoverContent
          align="end"
          :side-offset="6"
          class="z-50 w-64 rounded-xl border border-border bg-card p-3 text-card-foreground shadow-xl outline-none"
        >
          <div class="mb-2 flex items-center justify-between">
            <button class="rounded p-1 text-muted-foreground hover:bg-secondary" @click="prevMonth">
              <ChevronLeft class="h-4 w-4" />
            </button>
            <span class="text-sm font-medium">{{ monthLabel }} {{ viewYear }}</span>
            <button class="rounded p-1 text-muted-foreground hover:bg-secondary" @click="nextMonth">
              <ChevronRight class="h-4 w-4" />
            </button>
          </div>
          <div class="mb-1 grid grid-cols-7 gap-0.5 text-center text-[10px] font-medium text-muted-foreground">
            <span v-for="d in dows" :key="d">{{ d }}</span>
          </div>
          <div class="grid grid-cols-7 gap-0.5">
            <template v-for="(cell, i) in cells" :key="i">
              <button
                v-if="cell"
                class="h-7 rounded-md text-xs transition-colors"
                :class="isSelected(cell)
                  ? 'bg-primary text-primary-foreground'
                  : isToday(cell) ? 'font-semibold text-primary hover:bg-secondary' : 'hover:bg-secondary'"
                @click="pick(cell)"
              >{{ cell }}</button>
              <span v-else />
            </template>
          </div>
          <div class="mt-3 border-t border-border pt-3 text-sm">
            <div class="flex items-center gap-1.5">
              <span class="text-muted-foreground">Time</span>
              <input
                type="number" min="0" max="23" :value="pad(hour)"
                class="w-11 rounded-md border border-border bg-secondary/40 px-1 py-1 text-center text-foreground outline-none focus:border-primary/50"
                @input="hour = ($event.target as HTMLInputElement).valueAsNumber; apply()"
              />
              <span class="text-muted-foreground">:</span>
              <input
                type="number" min="0" max="59" :value="pad(minute)"
                class="w-11 rounded-md border border-border bg-secondary/40 px-1 py-1 text-center text-foreground outline-none focus:border-primary/50"
                @input="minute = ($event.target as HTMLInputElement).valueAsNumber; apply()"
              />
            </div>
            <div class="mt-2 flex items-center justify-between">
              <button
                v-if="modelValue > 0"
                class="whitespace-nowrap rounded-md px-2 py-1 text-xs text-muted-foreground hover:text-destructive"
                @click="clear(); open = false"
              >{{ t("notes.clear_reminder") }}</button>
              <span v-else />
              <button
                class="rounded-md bg-primary px-3 py-1 text-xs font-medium text-primary-foreground hover:bg-primary/90"
                @click="open = false"
              >Done</button>
            </div>
          </div>
        </PopoverContent>
      </PopoverPortal>
  </PopoverRoot>
</template>

<style scoped>
/* Drop the native up/down spinner on the HH:MM number inputs. */
input[type="number"]::-webkit-inner-spin-button,
input[type="number"]::-webkit-outer-spin-button {
  -webkit-appearance: none;
  appearance: none;
  margin: 0;
}
</style>
