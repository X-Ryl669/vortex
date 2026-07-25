<script setup lang="ts">
// First-run onboarding controller: top bar (logo + back + skip), progress bar,
// and the 9 step screens. Each step renders its own CTA and emits `next`; the
// final "All set" emits `start`, which marks the intro done and routes into the
// app (Devices → "Add phone"). The canvas is deliberately always dark so the
// illustrations read correctly regardless of the theme the user just picked.
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { ChevronLeft } from "lucide-vue-next";
import { markIntroDone } from "@/lib/intro";
import VortexMark from "./art/VortexMark.vue";
import LangTheme from "./steps/LangTheme.vue";
import WhatIs from "./steps/WhatIs.vue";
import FeatureSlide from "./steps/FeatureSlide.vue";
import Privacy from "./steps/Privacy.vue";
import AllSet from "./steps/AllSet.vue";
import "./onboarding.css";

const { t } = useI18n();
const router = useRouter();

const LAST = 10; // 0 lang/theme · 1 what-is · 2–8 features · 9 privacy · 10 all-set
const step = ref(0);

const current = computed(() => {
  if (step.value === 0) return LangTheme;
  if (step.value === 1) return WhatIs;
  if (step.value >= 2 && step.value <= 8) return FeatureSlide;
  if (step.value === 9) return Privacy;
  return AllSet;
});
const currentProps = computed(() =>
  step.value >= 2 && step.value <= 8 ? { index: step.value - 2 } : {},
);

const canBack = computed(() => step.value > 0 && step.value < LAST);
const canSkip = computed(() => step.value >= 2 && step.value <= 8);
const pct = computed(() => `${((step.value + 1) / (LAST + 1)) * 100}%`);

function go(n: number) {
  step.value = Math.max(0, Math.min(LAST, n));
}
function next() {
  go(step.value + 1);
}
function skip() {
  go(9); // jump to the privacy slide, then on to "all set"
}
function finish() {
  markIntroDone();
  void router.push("/");
}
</script>

<template>
  <div class="onb-root">
    <!-- top bar -->
    <div class="onb-top">
      <div class="onb-brand">
        <button v-if="canBack" class="onb-chip" @click="go(step - 1)">
          <ChevronLeft :size="17" :stroke-width="2" />
        </button>
        <VortexMark :size="26" />
        <span class="onb-wordmark">Vortex</span>
      </div>
      <button v-if="canSkip" class="onb-skip" @click="skip">{{ t("intro.skip") }}</button>
    </div>

    <!-- progress -->
    <div class="onb-progress"><div class="onb-bar" :style="{ width: pct }" /></div>

    <!-- content — the :key swap remounts each step so its own entrance
         animation plays instantly (no out-in gap, snappy yet smooth). -->
    <div class="onb-content">
      <component :is="current" v-bind="currentProps" :key="step" @next="next" @start="finish" />
    </div>
  </div>
</template>

<style scoped>
/* Ambient glow is baked into a STATIC multi-radial gradient (no blur filter,
   no animation) — under software rendering (DMABUF off) an animated blur cooks
   the CPU, while a static gradient costs essentially nothing. */
.onb-root { position: fixed; inset: 0; z-index: 50; display: flex; flex-direction: column; overflow: hidden; color: #F2F4F6; background-color: #0E0E10; background-image: radial-gradient(70% 55% at 18% 8%, rgba(46, 204, 113, 0.10), transparent 60%), radial-gradient(65% 55% at 88% 100%, rgba(46, 204, 113, 0.08), transparent 60%), radial-gradient(120% 70% at 50% -10%, rgba(46, 204, 113, 0.05), transparent 55%); }
.onb-top { display: flex; align-items: center; justify-content: space-between; padding: 18px 22px; flex: none; z-index: 5; position: relative; }
.onb-brand { display: flex; align-items: center; gap: 11px; }
.onb-chip { width: 32px; height: 32px; border-radius: 9px; display: flex; align-items: center; justify-content: center; background: rgba(255, 255, 255, 0.04); border: 1px solid rgba(255, 255, 255, 0.07); color: #C9CCD2; cursor: pointer; margin-right: 2px; transition: background 0.2s, color 0.2s; }
.onb-chip:hover { background: rgba(255, 255, 255, 0.09); color: #F2F4F6; }
.onb-wordmark { font-size: 16px; font-weight: 600; letter-spacing: -0.3px; }
.onb-skip { font-size: 13px; font-weight: 500; color: #8A8D93; background: transparent; border: none; cursor: pointer; font-family: inherit; padding: 8px 10px; border-radius: 8px; transition: background 0.2s, color 0.2s; }
.onb-skip:hover { background: rgba(255, 255, 255, 0.05); color: #C9CCD2; }
.onb-progress { height: 3px; background: rgba(255, 255, 255, 0.05); margin: 0 26px; border-radius: 3px; flex: none; overflow: hidden; position: relative; z-index: 5; }
.onb-bar { height: 100%; background: #2ECC71; border-radius: 3px; box-shadow: 0 0 10px rgba(46, 204, 113, 0.5); transition: width 0.5s cubic-bezier(0.22, 1, 0.36, 1); }
.onb-content { flex: 1; display: flex; align-items: center; justify-content: center; padding: 24px 40px 40px; position: relative; z-index: 1; min-height: 0; overflow-y: auto; }
</style>
