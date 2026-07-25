<script setup lang="ts">
// Step 7 — privacy: a pulsing shield, the "everything is yours" message.
import { useI18n } from "vue-i18n";

defineEmits<{ next: [] }>();
const { t } = useI18n();
</script>

<template>
  <div class="vxo-screen" style="max-width:520px;text-align:center;">
    <div class="halo-wrap">
      <div class="halo" />
      <div class="shield">
        <svg width="46" height="46" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" /><path d="M9 12l2 2 4-4" />
        </svg>
      </div>
    </div>
    <h1 style="font-size:30px;font-weight:600;letter-spacing:-.7px;margin-top:24px;">{{ t("intro.privacy_title") }}</h1>
    <p style="font-size:15px;color:#8A8D93;margin-top:13px;line-height:1.55;max-width:440px;margin-left:auto;margin-right:auto;">{{ t("intro.privacy_body") }}</p>
    <button class="vxo-pill" style="margin-top:30px;" @click="$emit('next')">{{ t("intro.continue") }}</button>
  </div>
</template>

<style scoped>
.halo-wrap { position: relative; width: 108px; height: 108px; margin: 0 auto; display: flex; align-items: center; justify-content: center; }
.halo { position: absolute; left: 50%; top: 50%; width: 108px; height: 108px; border-radius: 50%; border: 1px solid rgba(46, 204, 113, 0.4); animation: vxo-haloPulse 3s ease-out infinite; }
.shield { width: 96px; height: 96px; border-radius: 26px; display: flex; align-items: center; justify-content: center; background: rgba(46, 204, 113, 0.12); border: 1px solid rgba(46, 204, 113, 0.3); box-shadow: 0 0 40px rgba(46, 204, 113, 0.22); color: #2ECC71; }
</style>
