<script setup lang="ts">
// Step 1 — "what is Vortex": the hero device illustration + the one-liner.
import { useI18n } from "vue-i18n";
import DevicesArt from "../art/DevicesArt.vue";

defineEmits<{ next: [] }>();
const { t } = useI18n();
</script>

<template>
  <div class="vxo-screen" style="max-width:560px;text-align:center;">
    <DevicesArt />
    <h1 style="font-size:32px;font-weight:600;letter-spacing:-.8px;margin-top:20px;line-height:1.15;">{{ t("intro.whatis_title") }}</h1>
    <p style="font-size:15px;color:#8A8D93;margin-top:14px;line-height:1.55;">{{ t("intro.whatis_body") }}</p>
    <button class="vxo-pill" style="margin-top:30px;" @click="$emit('next')">{{ t("intro.continue") }}</button>
  </div>
</template>
