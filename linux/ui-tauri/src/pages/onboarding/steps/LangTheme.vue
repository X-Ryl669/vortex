<script setup lang="ts">
// Step 0 — language. Wired to the real app preference: setLocale() switches the
// UI language live (every string re-renders). Theme is left to Settings so the
// onboarding canvas can stay reliably dark for the illustrations.
import { useI18n } from "vue-i18n";
import { Check } from "lucide-vue-next";
import { setLocale, LOCALES, type LocaleCode } from "@/lib/i18n";
import VortexMark from "../art/VortexMark.vue";

defineEmits<{ next: [] }>();
const { t, locale } = useI18n();

function pickLang(code: LocaleCode) {
  setLocale(code);
}
</script>

<template>
  <div class="vxo-screen" style="max-width:440px;text-align:center;">
    <div class="logo-wrap">
      <span class="logo-ring" />
      <span class="logo-ring" style="animation-delay:1.4s;" />
      <VortexMark :size="56" pulse />
    </div>
    <h1 style="font-size:26px;font-weight:600;letter-spacing:-.5px;margin-top:18px;">{{ t("intro.welcome") }}</h1>
    <p style="font-size:14px;color:#8A8D93;margin-top:7px;">{{ t("intro.welcome_sub") }}</p>

    <!-- language -->
    <div style="text-align:left;margin-top:30px;">
      <div class="label">{{ t("intro.language") }}</div>
      <div style="display:flex;flex-direction:column;gap:9px;">
        <button
          v-for="(l, i) in LOCALES"
          :key="l.code"
          class="lang"
          :class="{ on: locale === l.code }"
          :style="{ animation: `vxo-rise .5s cubic-bezier(.34,1.3,.5,1) both ${0.12 + i * 0.07}s` }"
          @click="pickLang(l.code)"
        >
          <span>{{ l.label }}</span>
          <Check v-if="locale === l.code" :size="18" :stroke-width="2.4" style="color:#2ECC71;" />
        </button>
      </div>
    </div>

    <button class="vxo-pill" style="width:100%;margin-top:30px;" @click="$emit('next')">{{ t("intro.continue") }}</button>
  </div>
</template>

<style scoped>
.logo-wrap { position: relative; width: 56px; height: 56px; margin: 0 auto; display: flex; align-items: center; justify-content: center; }
.logo-ring { position: absolute; left: 50%; top: 50%; width: 84px; height: 84px; border-radius: 50%; border: 1px solid rgba(46, 204, 113, 0.35); transform: translate(-50%, -50%); animation: vxo-ring 2.8s ease-out infinite; }
.label { font-size: 11px; font-weight: 600; letter-spacing: 1px; text-transform: uppercase; color: #7E8189; margin-bottom: 10px; }
.lang { display: flex; align-items: center; justify-content: space-between; width: 100%; padding: 13px 16px; border-radius: 14px; cursor: pointer; font-family: inherit; font-size: 14.5px; font-weight: 500; text-align: left; transition: all 0.25s cubic-bezier(0.22, 1, 0.36, 1); color: #C9CCD2; background: rgba(255, 255, 255, 0.04); border: 1px solid rgba(255, 255, 255, 0.07); }
.lang.on { color: #F2F4F6; background: rgba(46, 204, 113, 0.12); border-color: rgba(46, 204, 113, 0.5); }
</style>
