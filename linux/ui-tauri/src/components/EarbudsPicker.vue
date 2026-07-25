<script setup lang="ts">
// The "+ Add earbuds" experience: a magical Bluetooth-scan sheet that
// matches the app's continuity-style pairing flow. A pulsing scanner orb,
// live device rows with signal bars + a connected badge, and a one-tap
// pick. Theme-aware via the app's CSS tokens. Mirrors PairingOverlay's
// visual language so the whole app feels of a piece.
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Headphones, Bluetooth, X, Loader2, RefreshCw, Check } from "lucide-vue-next";
import {
  earbudsAddOpen,
  earbudsScanning,
  earbudsScanResults,
  openEarbudsPicker,
  pickEarbud,
} from "@/composables/useHome";
import type { BluetoothDeviceRow } from "@/lib/bridge";

const { t } = useI18n();

// RSSI → 0..4 signal bars. Same qualitative buckets as the pairing radar.
function barsFor(rssi: number | null): number {
  const r = rssi ?? -100;
  return r > -55 ? 4 : r > -68 ? 3 : r > -80 ? 2 : 1;
}

type Row = BluetoothDeviceRow & { bars: number };
// Audio-class devices float to the top (the backend already sorts that
// way); we split so we can give the non-audio tail a quieter treatment.
const audioRows = computed<Row[]>(() =>
  earbudsScanResults.value
    .filter((d) => d.is_audio)
    .map((d) => ({ ...d, bars: barsFor(d.rssi) })),
);
const otherRows = computed<Row[]>(() =>
  earbudsScanResults.value
    .filter((d) => !d.is_audio)
    .map((d) => ({ ...d, bars: barsFor(d.rssi) })),
);
const hasResults = computed(() => earbudsScanResults.value.length > 0);

function close() {
  earbudsAddOpen.value = false;
}
</script>

<template>
  <Transition name="eb-fade">
    <div v-if="earbudsAddOpen" class="eb-backdrop" @click.self="close">
      <div class="eb-card">
        <button class="eb-x" :title="t('switch.cancel')" @click="close"><X :size="18" /></button>

        <!-- ░ SCANNER ORB ░ -->
        <div class="eb-orb">
          <span class="eb-orb-glow" />
          <span class="eb-pulse" />
          <span class="eb-pulse" style="animation-delay: 1.3s" />
          <span class="eb-pulse" style="animation-delay: 2.6s" />
          <span class="eb-orb-core"><Headphones :size="30" :stroke-width="1.7" /></span>
        </div>

        <div class="text-center" style="margin-top: 6px">
          <div class="eb-title">{{ t("earbuds.add") }}</div>
          <div class="eb-sub">{{ t("earbuds.add_modal_hint") }}</div>
        </div>

        <!-- status + rescan -->
        <div class="eb-status">
          <Loader2 v-if="earbudsScanning" :size="14" class="eb-spin" />
          <span v-else class="eb-status-dot" />
          <span>{{ earbudsScanning ? t("earbuds.scanning") : t("earbuds.scan_done") }}</span>
          <button class="eb-rescan" :disabled="earbudsScanning" @click="openEarbudsPicker">
            <RefreshCw :size="13" :class="{ 'eb-spin': earbudsScanning }" />
            {{ t("earbuds.rescan") }}
          </button>
        </div>

        <!-- device list -->
        <div class="eb-list">
          <button
            v-for="d in audioRows"
            :key="d.address"
            class="eb-row eb-row-audio"
            @click="pickEarbud(d)"
          >
            <span class="eb-row-icon eb-row-icon-audio"><Headphones :size="20" :stroke-width="1.8" /></span>
            <span class="min-w-0 flex-1 text-left">
              <span class="block text-sm font-semibold truncate">{{ d.name }}</span>
              <span class="eb-row-meta">
                <Check v-if="d.connected" :size="12" class="eb-connected-ic" />
                {{ d.connected ? t("earbuds.now_connected") : d.address }}
              </span>
            </span>
            <span class="eb-bars" :title="(d.rssi ?? '') + ' dBm'">
              <i v-for="n in 4" :key="n" :class="['eb-bar', { on: n <= d.bars }]" :style="{ height: 5 + n * 3 + 'px' }" />
            </span>
            <span class="eb-pick">{{ t("earbuds.use_here") }}</span>
          </button>

          <!-- non-audio tail, quieter -->
          <button
            v-for="d in otherRows"
            :key="d.address"
            class="eb-row"
            @click="pickEarbud(d)"
          >
            <span class="eb-row-icon"><Bluetooth :size="18" :stroke-width="1.8" /></span>
            <span class="min-w-0 flex-1 text-left">
              <span class="block text-sm font-medium truncate">{{ d.name }}</span>
              <span class="eb-row-meta">{{ d.address }}</span>
            </span>
            <span class="eb-bars">
              <i v-for="n in 4" :key="n" :class="['eb-bar', { on: n <= d.bars }]" :style="{ height: 5 + n * 3 + 'px' }" />
            </span>
          </button>

          <!-- empty / scanning placeholder -->
          <div v-if="!hasResults" class="eb-empty">
            <template v-if="earbudsScanning">
              <span class="eb-dot-pulse" />{{ t("earbuds.scanning") }}
            </template>
            <template v-else>{{ t("earbuds.scan_empty") }}</template>
          </div>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.eb-backdrop {
  position: fixed; inset: 0; z-index: 60; display: flex; align-items: center; justify-content: center;
  background: rgba(7, 8, 10, 0.55); backdrop-filter: blur(2px);
}
.eb-fade-enter-active, .eb-fade-leave-active { transition: opacity 0.3s ease; }
.eb-fade-enter-from, .eb-fade-leave-to { opacity: 0; }

.eb-card {
  position: relative; width: 460px; max-height: 88vh; overflow-y: auto;
  background: hsl(var(--card)); border: 1px solid hsl(var(--border));
  border-radius: 28px; padding: 34px 32px 28px; box-shadow: 0 30px 80px rgba(0, 0, 0, 0.5);
  animation: eb-modalIn 0.5s cubic-bezier(0.2, 1, 0.3, 1) both;
}
@keyframes eb-modalIn { 0% { transform: scale(0.92) translateY(14px); opacity: 0 } 100% { transform: scale(1) translateY(0); opacity: 1 } }

.eb-x {
  position: absolute; top: 16px; right: 16px; height: 32px; width: 32px; display: flex; align-items: center; justify-content: center;
  border-radius: 999px; color: hsl(var(--muted-foreground)); border: 1px solid hsl(var(--border)); background: hsl(var(--foreground) / 0.04); cursor: pointer;
  transition: background-color 0.28s ease, color 0.28s ease;
}
.eb-x:hover { color: hsl(var(--foreground)); background: hsl(var(--foreground) / 0.09); }

/* scanner orb */
.eb-orb { position: relative; width: 132px; height: 132px; margin: 6px auto 0; }
.eb-orb-glow {
  position: absolute; left: 50%; top: 50%; width: 124px; height: 124px; border-radius: 999px;
  background: radial-gradient(circle, hsl(var(--primary) / 0.34), transparent 66%);
  transform: translate(-50%, -50%); animation: eb-breathe 3.2s ease-in-out infinite;
}
@keyframes eb-breathe { 0%, 100% { opacity: 0.5; transform: translate(-50%, -50%) scale(0.92) } 50% { opacity: 0.85; transform: translate(-50%, -50%) scale(1.08) } }
.eb-pulse {
  position: absolute; left: 50%; top: 50%; width: 78px; height: 78px; border-radius: 999px;
  border: 1.5px solid hsl(var(--primary) / 0.5); transform: translate(-50%, -50%);
  animation: eb-pulse 3.9s ease-out infinite;
}
@keyframes eb-pulse { 0% { transform: translate(-50%, -50%) scale(0.5); opacity: 0.6 } 100% { transform: translate(-50%, -50%) scale(1.7); opacity: 0 } }
.eb-orb-core {
  position: absolute; left: 50%; top: 50%; transform: translate(-50%, -50%); width: 64px; height: 64px;
  border-radius: 20px; background: hsl(var(--secondary)); border: 1px solid hsl(var(--primary) / 0.45);
  display: flex; align-items: center; justify-content: center; color: hsl(var(--primary));
  box-shadow: 0 0 30px hsl(var(--primary) / 0.35); animation: eb-floaty 3.4s ease-in-out infinite; z-index: 2;
}
@keyframes eb-floaty { 0%, 100% { transform: translate(-50%, -50%) } 50% { transform: translate(-50%, calc(-50% - 6px)) } }

.eb-title { font-size: 21px; font-weight: 700; letter-spacing: -0.4px; color: hsl(var(--foreground)); margin-top: 14px; }
.eb-sub { font-size: 13px; color: hsl(var(--muted-foreground)); margin: 7px auto 0; max-width: 320px; line-height: 1.5; }

/* status row */
.eb-status {
  display: flex; align-items: center; gap: 8px; margin-top: 20px; padding: 0 2px;
  font-size: 12.5px; color: hsl(var(--muted-foreground));
}
.eb-status-dot { width: 8px; height: 8px; border-radius: 999px; background: hsl(var(--primary)); animation: eb-blink 1.6s ease-in-out infinite; }
@keyframes eb-blink { 0%, 100% { opacity: 0.35 } 50% { opacity: 1 } }
.eb-rescan {
  margin-left: auto; display: inline-flex; align-items: center; gap: 5px; padding: 5px 11px; border-radius: 999px;
  font-size: 12px; font-weight: 600; color: hsl(var(--primary)); background: hsl(var(--primary) / 0.1);
  border: none; cursor: pointer; font-family: inherit; transition: background-color 0.26s ease, opacity 0.26s ease;
}
.eb-rescan:hover { background: hsl(var(--primary) / 0.18); }
.eb-rescan:disabled { opacity: 0.5; cursor: default; }
.eb-spin { animation: eb-rot 0.9s linear infinite; }
@keyframes eb-rot { from { transform: rotate(0deg) } to { transform: rotate(360deg) } }

/* device list */
.eb-list { display: flex; flex-direction: column; gap: 9px; margin-top: 14px; }
.eb-row {
  display: flex; align-items: center; gap: 12px; padding: 11px 13px; border-radius: 15px;
  background: hsl(var(--foreground) / 0.04); border: 1px solid hsl(var(--border)); cursor: pointer; width: 100%;
  animation: eb-rowIn 0.5s cubic-bezier(0.2, 1, 0.3, 1) both;
  transition: background-color 0.26s ease, border-color 0.26s ease, transform 0.26s cubic-bezier(0.22, 1, 0.36, 1);
}
.eb-row:hover { background: hsl(var(--foreground) / 0.08); border-color: hsl(var(--primary) / 0.4); }
.eb-row:active { transform: scale(0.99); }
.eb-row-audio { background: hsl(var(--primary) / 0.06); border-color: hsl(var(--primary) / 0.25); }
.eb-row-audio:hover { background: hsl(var(--primary) / 0.1); }
@keyframes eb-rowIn { 0% { transform: translateY(8px); opacity: 0 } 100% { transform: translateY(0); opacity: 1 } }
.eb-row-icon {
  width: 40px; height: 40px; border-radius: 12px; flex: none; display: flex; align-items: center; justify-content: center;
  background: hsl(var(--foreground) / 0.05); border: 1px solid hsl(var(--border)); color: hsl(var(--muted-foreground));
}
.eb-row-icon-audio { background: hsl(var(--primary) / 0.12); border-color: hsl(var(--primary) / 0.3); color: hsl(var(--primary)); }
.eb-row-meta { display: inline-flex; align-items: center; gap: 4px; font-size: 11px; color: hsl(var(--muted-foreground)); margin-top: 1px; }
.eb-connected-ic { color: hsl(var(--primary)); }
.eb-bars { display: flex; align-items: flex-end; gap: 2px; height: 18px; flex: none; }
.eb-bar { width: 4px; border-radius: 2px; background: hsl(var(--foreground) / 0.14); }
.eb-bar.on { background: hsl(var(--primary)); }
.eb-pick {
  padding: 7px 14px; border-radius: 999px; background: hsl(var(--primary)); color: hsl(var(--primary-foreground));
  font-size: 12px; font-weight: 700; flex: none;
  opacity: 0; transform: translateX(6px); transition: opacity 0.26s ease, transform 0.26s cubic-bezier(0.22, 1, 0.36, 1);
}
.eb-row-audio:hover .eb-pick { opacity: 1; transform: translateX(0); }

.eb-empty {
  display: flex; align-items: center; justify-content: center; gap: 8px; padding: 18px;
  font-size: 12.5px; color: hsl(var(--muted-foreground)); text-align: center; line-height: 1.5;
}
.eb-dot-pulse { width: 8px; height: 8px; border-radius: 999px; background: hsl(var(--primary)); animation: eb-blink 1.4s ease-in-out infinite; }
</style>
