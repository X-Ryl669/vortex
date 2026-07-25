<script setup lang="ts">
// Steps 2–8 — one feature per slide: animated art + title + body. Data-driven
// by `index` (0–6) so all seven features share this one component.
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import EarbudsArt from "../art/EarbudsArt.vue";
import PhoneNotifArt from "../art/PhoneNotifArt.vue";
import ClipboardArt from "../art/ClipboardArt.vue";
import HandoffArt from "../art/HandoffArt.vue";
import FileShareArt from "../art/FileShareArt.vue";
import MirrorArt from "../art/MirrorArt.vue";
import ProximityArt from "../art/ProximityArt.vue";

const props = defineProps<{ index: number }>();
defineEmits<{ next: [] }>();
const { t } = useI18n();

const FEATURES = [
  { art: EarbudsArt, key: "earbuds" },
  { art: PhoneNotifArt, key: "phone" },
  { art: ClipboardArt, key: "clipboard" },
  { art: HandoffArt, key: "handoff" },
  { art: FileShareArt, key: "files" },
  { art: MirrorArt, key: "mirror" },
  { art: ProximityArt, key: "proximity" },
] as const;

const feature = computed(() => FEATURES[props.index] ?? FEATURES[0]);
</script>

<template>
  <!-- Fixed-height regions (art + copy) keep the Next button pinned to the same
       spot on every slide, so it never jumps as the title/body length changes. -->
  <div class="vxo-screen feat">
    <!-- :key re-mounts the art each slide so its entrance animation replays -->
    <div class="art-wrap">
      <span class="art-glow" />
      <component :is="feature.art" :key="feature.key" />
    </div>
    <div class="copy">
      <h1 class="title">{{ t(`intro.feat.${feature.key}_title`) }}</h1>
      <p class="body">{{ t(`intro.feat.${feature.key}_body`) }}</p>
    </div>
    <button class="vxo-pill" @click="$emit('next')">{{ t("intro.next") }}</button>
  </div>
</template>

<style scoped>
.feat { max-width: 520px; display: flex; flex-direction: column; align-items: center; text-align: center; }
.art-wrap { position: relative; height: 156px; display: flex; align-items: center; justify-content: center; flex: none; }
.art-glow { position: absolute; left: 50%; top: 50%; width: 320px; height: 170px; transform: translate(-50%, -50%); border-radius: 50%; background: radial-gradient(ellipse, rgba(46, 204, 113, 0.14), transparent 68%); pointer-events: none; z-index: 0; }
.copy { height: 128px; margin-top: 22px; display: flex; flex-direction: column; align-items: center; flex: none; }
.title { font-size: 25px; font-weight: 600; letter-spacing: -0.5px; }
.body { font-size: 14.5px; color: #8A8D93; margin-top: 11px; line-height: 1.55; max-width: 420px; }
.vxo-pill { margin-top: 4px; }
</style>
