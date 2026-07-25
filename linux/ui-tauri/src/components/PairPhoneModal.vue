<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { Smartphone, Loader2 } from "lucide-vue-next";
import Modal from "@/components/Modal.vue";
import SignalBars from "@/components/SignalBars.vue";
import type { ScanHit } from "@/lib/bridge";

defineProps<{ open: boolean; scanning: boolean; hits: ScanHit[] }>();
const emit = defineEmits<{ dismiss: []; pair: [addr: string] }>();
const { t } = useI18n();
</script>

<template>
  <Modal :open="open" :title="t('pair.add_phone')" @dismiss="emit('dismiss')">
    <div class="space-y-3">
      <p class="text-sm text-muted-foreground">
        {{ t("pair.add_phone_modal_hint") }}
      </p>
      <div class="flex items-center gap-2 text-xs text-muted-foreground">
        <Loader2 v-if="scanning" class="h-3.5 w-3.5 animate-spin" />
        <span v-else class="h-2 w-2 rounded-full bg-emerald-500 animate-pulse" />
        <span>{{ scanning ? t("scan.scanning") : t("discover.looking") }}</span>
      </div>

      <ul v-if="hits.length > 0" class="space-y-2">
        <li v-for="h in hits" :key="h.instance">
          <button
            class="w-full flex items-center justify-between rounded-lg border border-border bg-background hover:bg-accent px-3 py-2.5 text-left transition-colors"
            @click="emit('pair', h.addr)"
          >
            <div class="flex items-center gap-2.5">
              <div class="h-9 w-9 rounded-md bg-primary/10 text-primary flex items-center justify-center">
                <Smartphone class="h-4.5 w-4.5" />
              </div>
              <div class="text-sm font-medium">{{ h.name || t("device.android") }}</div>
            </div>
            <SignalBars :rssi="h.rssi" />
          </button>
        </li>
      </ul>
      <div v-else class="text-sm text-muted-foreground py-2 text-center">
        {{ t("discover.looking") }}
      </div>
    </div>
  </Modal>
</template>
