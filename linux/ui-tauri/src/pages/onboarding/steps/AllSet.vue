<script setup lang="ts">
// Final step — "you're ready". The button finishes the intro; the parent
// marks it done and routes to the Devices page (where "Add phone" lives).
// One firm callout before pairing: other BLE phone-link apps
// hold the same radio and can block Vortex's connection, so close them.
import { useI18n } from "vue-i18n";
import { Bluetooth } from "lucide-vue-next";

defineEmits<{ start: [] }>();
const { t } = useI18n();
</script>

<template>
  <div class="vxo-screen" style="max-width:480px;text-align:center;">
    <div class="check">
      <svg width="50" height="50" viewBox="0 0 24 24" fill="none" stroke="#07120D" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
        <path d="M20 6 9 17l-5-5" />
      </svg>
    </div>
    <h1 style="font-size:30px;font-weight:600;letter-spacing:-.7px;margin-top:26px;">{{ t("intro.ready_title") }}</h1>
    <p style="font-size:15px;color:#8A8D93;margin-top:12px;">{{ t("intro.ready_body") }}</p>

    <div class="ble-note">
      <Bluetooth :size="18" :stroke-width="2.2" class="ble-ic" />
      <div class="ble-text">
        <div class="ble-title">{{ t("intro.ble_tip_title") }}</div>
        <div class="ble-body">{{ t("intro.ble_tip_body") }}</div>
      </div>
    </div>

    <button class="vxo-pill" style="margin-top:24px;padding:14px 36px;font-size:15px;" @click="$emit('start')">{{ t("intro.start") }}</button>
  </div>
</template>

<style scoped>
.check { width: 104px; height: 104px; border-radius: 50%; margin: 0 auto; display: flex; align-items: center; justify-content: center; background: #2ECC71; box-shadow: 0 12px 40px rgba(46, 204, 113, 0.45); animation: vxo-checkPop 0.6s cubic-bezier(0.34, 1.4, 0.5, 1) both; }
.ble-note { display: flex; gap: 12px; text-align: left; margin-top: 28px; padding: 14px 16px; border-radius: 14px; background: rgba(255, 200, 60, 0.07); border: 1px solid rgba(255, 200, 60, 0.22); }
.ble-ic { color: #F0B43C; flex: none; margin-top: 1px; }
.ble-title { font-size: 13.5px; font-weight: 600; color: #F2F4F6; }
.ble-body { font-size: 13px; color: #9DA0A6; margin-top: 4px; line-height: 1.5; }
</style>
