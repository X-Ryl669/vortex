<script setup lang="ts">
// The full-screen, continuity-style pairing experience: radar scan →
// connecting → emoji security check → success / aborted. Driven by the real
// pairing state in useHome; ported from the Vortex Pairing design. Theme-aware
// via the app's CSS tokens (dark/light handled globally).
import { computed, watch, onUnmounted } from "vue";
import { useI18n } from "vue-i18n";
import { Laptop, Smartphone, ShieldCheck, AlertTriangle, Loader2, X } from "lucide-vue-next";
import {
  showPairPhoneModal,
  scanning,
  scanHits,
  pairingPeer,
  pairingSas,
  pairingResult,
  pairLocalRejected,
  pairWith,
  dismissPairing,
  proximityHit,
  pairFromProximity,
  dismissProximity,
} from "@/composables/useHome";
import { sasToGlyphs } from "@/lib/sasEmoji";

const { t } = useI18n();

// Which screen of the flow is live. Derived purely from the pairing state so it
// always reflects the backend.
type Screen = "proximity" | "scan" | "connecting" | "emoji" | "success" | "aborted" | null;
const screen = computed<Screen>(() => {
  if (pairingPeer.value) {
    if (pairLocalRejected.value) return "aborted";
    if (pairingResult.value) return pairingResult.value.ok ? "success" : "aborted";
    if (pairingSas.value) return "emoji";
    return "connecting";
  }
  if (proximityHit.value) return "proximity";
  if (showPairPhoneModal.value) return "scan";
  return null;
});

const proximityName = computed(() => proximityHit.value?.name || t("device.android"));

// Auto-dismiss the success screen after a beat — no "Done" button needed.
let successTimer: ReturnType<typeof setTimeout> | undefined;
watch(screen, (s) => {
  clearTimeout(successTimer);
  if (s === "success") successTimer = setTimeout(() => dismissPairing(), 3000);
});
onUnmounted(() => clearTimeout(successTimer));

const glyphs = computed(() => (pairingSas.value ? sasToGlyphs(pairingSas.value) : []));

// Radar rows from live BLE scan hits. RSSI → qualitative proximity + bars.
const radarDevices = computed(() =>
  scanHits.value.map((h) => {
    const rssi = h.rssi ?? -100;
    const bars = rssi > -55 ? 4 : rssi > -68 ? 3 : rssi > -80 ? 2 : 1;
    const meta = bars >= 4 ? t("scan.very_close") : bars >= 3 ? t("scan.near") : t("scan.far");
    return { addr: h.addr, name: h.name || t("device.android"), bars, meta };
  }),
);

function close() {
  showPairPhoneModal.value = false;
  if (pairingPeer.value) dismissPairing();
}
// Click on the dimmed backdrop: dismiss the scan modal or the proximity sheet
// (never a mid-handshake screen — those need an explicit decision).
function onBackdrop() {
  if (screen.value === "scan") close();
  else if (screen.value === "proximity") dismissProximity();
}
// Confetti particles for the success burst.
const confetti = Array.from({ length: 22 }, (_, i) => {
  const ang = (Math.PI * 2 * i) / 22 + (i % 3) * 0.2;
  const dist = 90 + (i % 5) * 26;
  return {
    tx: (Math.cos(ang) * dist).toFixed(0) + "px",
    ty: (Math.sin(ang) * dist - 30).toFixed(0) + "px",
    rot: i * 40 + "deg",
    round: i % 3 === 0,
    big: i % 2 === 0,
    delay: (0.35 + (i % 5) * 0.04).toFixed(2) + "s",
    dur: (1 + (i % 4) * 0.18).toFixed(2) + "s",
    color: ["#2ECC71", "#5BE2A0", "#A8F0CE", "#1FBF73"][i % 4],
  };
});
</script>

<template>
  <Transition name="vx-fade">
    <div v-if="screen" class="vp-backdrop" :class="{ 'vp-backdrop--sheet': screen === 'proximity' }" @click.self="onBackdrop">

      <!-- ░ PROXIMITY SHEET (AirPods-style, slides up) ░ -->
      <div v-if="screen === 'proximity'" class="vp-sheet">
        <div class="vp-grabber" />
        <div class="vp-phone-illus">
          <span class="vp-phone-glow" />
          <span class="vp-phone-ring" />
          <span class="vp-phone-ring" style="animation-delay: 1.4s" />
          <span class="vp-phone-body"><span class="vp-phone-screen"><span class="vp-phone-dot" /></span></span>
        </div>
        <div class="text-center" style="margin-top: 14px">
          <div class="vp-title">{{ t("pair.prox_title", { name: proximityName }) }}</div>
          <div class="vp-sub"><ShieldCheck :size="14" />{{ t("pair.prox_sub") }}</div>
        </div>
        <button class="vp-btn vp-btn-primary" style="margin-top: 24px" @click="pairFromProximity">{{ t("pair.add_phone_btn") }}</button>
        <button class="vp-btn vp-btn-quiet" @click="dismissProximity">{{ t("pair.not_now") }}</button>
      </div>

      <!-- ░ RADAR SCAN ░ -->
      <div v-if="screen === 'scan'" class="vp-card" style="width: 560px">
        <button class="vp-x" :title="t('scan.close')" @click="close"><X :size="18" /></button>
        <div class="text-center">
          <div class="vp-title">{{ t("scan.looking") }}</div>
          <div class="vp-sub">{{ t("scan.hint_open") }}</div>
        </div>
        <div class="vp-radar">
          <div class="vp-radar-disk">
            <span class="vp-ring" style="width: 158px; height: 158px" />
            <span class="vp-ring" style="width: 80px; height: 80px" />
            <span class="vp-cross-v" /><span class="vp-cross-h" />
            <span class="vp-radar-glow" />
            <span class="vp-sweep" />
          </div>
          <div class="vp-radar-core"><div class="vp-logo-dot" /></div>
        </div>
        <div class="vp-list">
          <div v-if="radarDevices.length === 0" class="vp-empty">
            <span class="vp-dot-pulse" />{{ scanning ? t("scan.scanning") : t("scan.empty") }}
          </div>
          <button
            v-for="d in radarDevices"
            :key="d.addr"
            class="vp-row"
            @click="pairWith(d.addr)"
          >
            <span class="vp-row-icon"><Smartphone :size="20" :stroke-width="1.7" /></span>
            <span class="min-w-0 flex-1 text-left">
              <span class="block text-sm font-semibold truncate">{{ d.name }}</span>
              <span class="block text-xs text-muted-foreground">{{ d.meta }}</span>
            </span>
            <span class="vp-bars">
              <i v-for="n in 4" :key="n" :class="['vp-bar', { on: n <= d.bars }]" :style="{ height: 5 + n * 3 + 'px' }" />
            </span>
            <span class="vp-pill">{{ t("pair.add_phone_btn") }}</span>
          </button>
        </div>
      </div>

      <!-- ░ CONNECTING ░ -->
      <div v-else-if="screen === 'connecting'" class="vp-card text-center" style="width: 460px">
        <div class="vp-link">
          <span class="vp-halo" style="left: 70px" />
          <span class="vp-halo" style="left: 230px; animation-delay: 1.4s" />
          <svg width="300" height="120" style="position: absolute; inset: 0; overflow: visible">
            <line x1="70" y1="60" x2="230" y2="60" stroke="hsl(var(--border))" stroke-width="3" stroke-linecap="round" />
            <line x1="70" y1="60" x2="230" y2="60" stroke="hsl(var(--primary))" stroke-width="3" stroke-linecap="round" stroke-dasharray="3 9" class="vp-flow" />
          </svg>
          <span class="vp-packet" />
          <span class="vp-node vp-node-l"><Laptop :size="26" :stroke-width="1.7" /></span>
          <span class="vp-node vp-node-r"><Smartphone :size="26" :stroke-width="1.8" /></span>
          <span class="vp-locktag"><ShieldCheck :size="13" :stroke-width="2.2" /></span>
        </div>
        <div class="vp-title-sm">{{ t("pair.connecting") }}</div>
        <div class="vp-sub vp-glow"><ShieldCheck :size="13" class="text-primary" />{{ t("pair.e2e") }}</div>
      </div>

      <!-- ░ EMOJI SECURITY CHECK ░ -->
      <div v-else-if="screen === 'emoji'" class="vp-card text-center" style="width: 500px">
        <span class="vp-badge"><ShieldCheck :size="13" :stroke-width="2" />{{ t("pair.sec_check") }}</span>
        <div class="vp-title" style="margin-top: 16px">{{ t("pair.emoji_title") }}</div>
        <div class="vp-sub" style="max-width: 380px; margin: 9px auto 0; line-height: 1.5">{{ t("pair.emoji_body") }}</div>
        <div class="vp-tiles">
          <div v-for="(g, i) in glyphs" :key="i" class="vp-tile-wrap">
            <div class="vp-tile" :style="{ animationDelay: i * 0.15 + 's' }">{{ g.emoji }}</div>
            <span class="vp-tile-name">{{ g.name }}</span>
          </div>
        </div>
        <!-- The phone is the decision point: it shows the same 3 emoji with
             Approve / Decline. The laptop just waits. -->
        <div class="vp-waiting">
          <Loader2 :size="16" class="vp-spin" />{{ t("pair.awaiting_phone") }}
        </div>
      </div>

      <!-- ░ SUCCESS ░ -->
      <div v-else-if="screen === 'success'" class="vp-card text-center" style="width: 440px; overflow: hidden">
        <div class="vp-confetti-origin">
          <span
            v-for="(c, i) in confetti"
            :key="i"
            class="vp-confetti"
            :class="{ round: c.round, big: c.big }"
            :style="{ '--tx': c.tx, '--ty': c.ty, '--rot': c.rot, background: c.color, animationDelay: c.delay, animationDuration: c.dur }"
          />
        </div>
        <div class="vp-check">
          <span class="vp-check-ring" />
          <svg width="104" height="104" viewBox="0 0 104 104" style="position: absolute; inset: 0">
            <path d="M32 54 L46 68 L73 38" fill="none" stroke="hsl(var(--primary))" stroke-width="6" stroke-linecap="round" stroke-linejoin="round" stroke-dasharray="60" class="vp-check-draw" />
          </svg>
        </div>
        <div class="vp-title" style="margin-top: 24px">{{ t("pair.success") }}</div>
        <div class="vp-sub">{{ t("pair.success_body") }}</div>
      </div>

      <!-- ░ ABORTED (mismatch / possible MITM) ░ -->
      <div v-else-if="screen === 'aborted'" class="vp-card text-center vp-card-danger" style="width: 460px">
        <div class="vp-warn"><AlertTriangle :size="36" :stroke-width="1.9" /></div>
        <div class="vp-title" style="margin-top: 20px">{{ t("pair.abort_title") }}</div>
        <div class="vp-sub" style="max-width: 380px; margin: 9px auto 0; line-height: 1.55">{{ t("pair.abort_body") }}</div>
        <button class="vp-btn vp-btn-primary" style="margin-top: 24px" @click="dismissPairing">{{ t("pair.start_over") }}</button>
      </div>

    </div>
  </Transition>
</template>

<style scoped>
.vp-backdrop {
  position: fixed; inset: 0; z-index: 60; display: flex; align-items: center; justify-content: center;
  background: rgba(7, 8, 10, 0.55); backdrop-filter: blur(2px);
}
.vp-fade-enter-active, .vp-fade-leave-active { transition: opacity 0.3s ease; }
.vp-fade-enter-from, .vp-fade-leave-to { opacity: 0; }

.vp-card {
  position: relative; background: hsl(var(--card)); border: 1px solid hsl(var(--border));
  border-radius: 28px; padding: 40px; box-shadow: 0 30px 80px rgba(0, 0, 0, 0.5);
  animation: vp-modalIn 0.5s cubic-bezier(0.2, 1, 0.3, 1) both;
}
.vp-card-danger { border-color: hsl(0 70% 55% / 0.35); }
@keyframes vp-modalIn { 0% { transform: scale(0.92) translateY(14px); opacity: 0 } 100% { transform: scale(1) translateY(0); opacity: 1 } }

/* proximity bottom sheet */
.vp-backdrop--sheet { align-items: flex-end; }
.vp-sheet {
  width: 480px; background: hsl(var(--card)); border: 1px solid hsl(var(--border)); border-bottom: none;
  border-radius: 28px 28px 0 0; padding: 24px 36px 40px; box-shadow: 0 -24px 70px rgba(0, 0, 0, 0.5);
  animation: vp-sheetUp 0.62s cubic-bezier(0.18, 1.1, 0.3, 1) both;
}
@keyframes vp-sheetUp { 0% { transform: translateY(105%); opacity: 0.4 } 100% { transform: translateY(0); opacity: 1 } }
.vp-grabber { width: 42px; height: 5px; border-radius: 999px; background: hsl(var(--foreground) / 0.12); margin: 0 auto 22px; }
.vp-phone-illus { position: relative; width: 140px; height: 140px; margin: 0 auto; }
.vp-phone-glow { position: absolute; left: 50%; top: 50%; width: 130px; height: 130px; border-radius: 999px; background: radial-gradient(circle, hsl(var(--primary) / 0.4), transparent 68%); animation: vp-glowPulse 3s ease-in-out infinite; }
@keyframes vp-glowPulse { 0%, 100% { opacity: 0.5; transform: translate(-50%, -50%) scale(0.95) } 50% { opacity: 0.85; transform: translate(-50%, -50%) scale(1.08) } }
.vp-phone-ring { position: absolute; left: 50%; top: 50%; width: 90px; height: 90px; border-radius: 999px; border: 1.5px solid hsl(var(--primary) / 0.5); animation: vp-ringPulse 2.8s ease-out infinite; }
@keyframes vp-ringPulse { 0% { transform: translate(-50%, -50%) scale(0.45); opacity: 0.55 } 100% { transform: translate(-50%, -50%) scale(1.85); opacity: 0 } }
.vp-phone-body { position: absolute; left: 50%; top: 50%; transform: translate(-50%, -50%); width: 62px; height: 96px; border-radius: 15px; background: hsl(var(--secondary)); border: 1.5px solid hsl(var(--border)); display: flex; align-items: center; justify-content: center; animation: vp-floaty 3.4s ease-in-out infinite; box-shadow: 0 12px 30px rgba(0, 0, 0, 0.35); }
@keyframes vp-floaty { 0%, 100% { transform: translate(-50%, -50%) } 50% { transform: translate(-50%, calc(-50% - 8px)) } }
.vp-phone-screen { width: 42px; height: 74px; border-radius: 9px; background: linear-gradient(160deg, hsl(var(--primary) / 0.22), hsl(var(--primary) / 0.06)); display: flex; align-items: flex-start; justify-content: center; padding-top: 5px; }
.vp-phone-dot { width: 4px; height: 4px; border-radius: 999px; background: hsl(var(--primary)); opacity: 0.85; }

.vp-x {
  position: absolute; top: 16px; right: 16px; height: 32px; width: 32px; display: flex; align-items: center; justify-content: center;
  border-radius: 999px; color: hsl(var(--muted-foreground)); border: 1px solid hsl(var(--border)); background: hsl(var(--foreground) / 0.04); cursor: pointer;
  transition: background-color 0.28s ease, color 0.28s ease;
}
.vp-x:hover { color: hsl(var(--foreground)); background: hsl(var(--foreground) / 0.09); }

.vp-title { font-size: 23px; font-weight: 700; letter-spacing: -0.4px; color: hsl(var(--foreground)); }
.vp-title-sm { font-size: 19px; font-weight: 600; letter-spacing: -0.3px; color: hsl(var(--foreground)); margin-top: 26px; }
.vp-sub { display: inline-flex; align-items: center; gap: 7px; font-size: 13.5px; color: hsl(var(--muted-foreground)); margin-top: 7px; }
.vp-glow { animation: vp-textGlow 2.4s ease-in-out infinite; }
@keyframes vp-textGlow { 0%, 100% { opacity: 0.6 } 50% { opacity: 1 } }

/* badge */
.vp-badge {
  display: inline-flex; align-items: center; gap: 7px; padding: 6px 13px; border-radius: 999px;
  background: hsl(var(--primary) / 0.12); color: hsl(var(--primary)); font-size: 11.5px; font-weight: 700; letter-spacing: 0.4px;
}

/* emoji tiles */
.vp-tiles { display: flex; justify-content: center; gap: 16px; margin-top: 30px; }
.vp-tile-wrap { display: flex; flex-direction: column; align-items: center; gap: 10px; }
.vp-tile {
  width: 100px; height: 104px; border-radius: 22px; background: hsl(var(--secondary)); border: 1px solid hsl(var(--border));
  display: flex; align-items: center; justify-content: center; font-size: 46px;
  box-shadow: 0 14px 30px rgba(0, 0, 0, 0.2); animation: vp-floatTile 3.4s ease-in-out infinite;
}
.vp-tile-name { font-size: 12.5px; font-weight: 600; color: hsl(var(--muted-foreground)); letter-spacing: 0.2px; }
@keyframes vp-floatTile { 0%, 100% { transform: translateY(0) } 50% { transform: translateY(-7px) } }

/* buttons — smooth, springy hover */
.vp-btn { width: 100%; border-radius: 999px; font-size: 14.5px; font-weight: 700; cursor: pointer; font-family: inherit; border: none; transition: transform 0.32s cubic-bezier(0.22, 1, 0.36, 1), filter 0.32s ease, background-color 0.32s ease, color 0.32s ease, box-shadow 0.32s ease; }
.vp-btn-primary { margin-top: 32px; padding: 15px; background: hsl(var(--primary)); color: hsl(var(--primary-foreground)); box-shadow: 0 10px 26px hsl(var(--primary) / 0.35); }
.vp-btn-primary:hover { transform: translateY(-2px); filter: brightness(1.06); box-shadow: 0 16px 34px hsl(var(--primary) / 0.5); }
.vp-btn-primary:active { transform: translateY(0); filter: brightness(0.98); }
.vp-btn-quiet { margin-top: 10px; padding: 12px; background: transparent; color: hsl(var(--muted-foreground)); font-weight: 600; font-size: 13px; }
.vp-btn-quiet:hover { background: hsl(var(--foreground) / 0.07); color: hsl(var(--foreground)); }
.vp-btn-quiet:active { background: hsl(var(--foreground) / 0.1); }

/* waiting indicator (emoji screen) */
.vp-waiting { display: inline-flex; align-items: center; gap: 8px; margin-top: 28px; font-size: 13.5px; font-weight: 600; color: hsl(var(--muted-foreground)); }
.vp-spin { animation: vp-rot 0.9s linear infinite; color: hsl(var(--primary)); }
@keyframes vp-rot { from { transform: rotate(0deg) } to { transform: rotate(360deg) } }

/* connecting */
.vp-link { position: relative; width: 300px; height: 120px; margin: 0 auto; }
.vp-halo { position: absolute; top: 60px; width: 104px; height: 104px; border-radius: 999px; transform: translate(-50%, -50%); background: radial-gradient(circle, hsl(var(--primary) / 0.3), transparent 68%); animation: vp-halo 2.8s ease-in-out infinite; }
@keyframes vp-halo { 0%, 100% { transform: translate(-50%, -50%) scale(0.85); opacity: 0.35 } 50% { transform: translate(-50%, -50%) scale(1.15); opacity: 0.7 } }
.vp-flow { animation: vp-flow 0.7s linear infinite; }
@keyframes vp-flow { to { stroke-dashoffset: -26 } }
.vp-packet { position: absolute; top: 60px; width: 13px; height: 13px; border-radius: 999px; background: #eafff4; box-shadow: 0 0 12px 3px hsl(var(--primary) / 0.8); transform: translate(-50%, -50%); animation: vp-travel 1.9s cubic-bezier(0.45, 0, 0.55, 1) infinite; }
@keyframes vp-travel { 0% { left: 70px; opacity: 0 } 12% { opacity: 1 } 88% { opacity: 1 } 100% { left: 230px; opacity: 0 } }
.vp-node { position: absolute; top: 60px; transform: translate(-50%, -50%); width: 60px; height: 60px; border-radius: 18px; display: flex; align-items: center; justify-content: center; }
.vp-node-l { left: 70px; background: hsl(var(--secondary)); border: 1px solid hsl(var(--border)); color: hsl(var(--foreground)); box-shadow: 0 8px 22px rgba(0, 0, 0, 0.25); }
.vp-node-r { left: 230px; background: hsl(var(--primary)); color: hsl(var(--primary-foreground)); box-shadow: 0 8px 24px hsl(var(--primary) / 0.5); }
.vp-locktag { position: absolute; left: 150px; top: 24px; transform: translate(-50%, 0); width: 26px; height: 26px; border-radius: 8px; background: hsl(var(--card)); border: 1px solid hsl(var(--primary) / 0.4); display: flex; align-items: center; justify-content: center; color: hsl(var(--primary)); animation: vp-bob 2.2s ease-in-out infinite; }
@keyframes vp-bob { 0%, 100% { transform: translate(-50%, 0) } 50% { transform: translate(-50%, -3px) } }

/* check / success */
.vp-check { position: relative; width: 104px; height: 104px; margin: 0 auto; animation: vp-pop 0.55s cubic-bezier(0.2, 1.4, 0.4, 1) both; }
.vp-check-ring { position: absolute; inset: 0; border-radius: 999px; background: hsl(var(--primary) / 0.14); border: 1.5px solid hsl(var(--primary) / 0.4); }
.vp-check-draw { animation: vp-draw 0.5s cubic-bezier(0.6, 0, 0.3, 1) 0.35s both; }
@keyframes vp-draw { from { stroke-dashoffset: 60 } to { stroke-dashoffset: 0 } }
@keyframes vp-pop { 0% { transform: scale(0.3); opacity: 0 } 55% { transform: scale(1.12); opacity: 1 } 100% { transform: scale(1); opacity: 1 } }
.vp-confetti-origin { position: absolute; left: 50%; top: 118px; width: 1px; height: 1px; }
.vp-confetti { position: absolute; left: 0; top: 0; width: 7px; height: 7px; border-radius: 2px; animation: vp-confetti cubic-bezier(0.2, 0.7, 0.3, 1) both; }
.vp-confetti.big { width: 9px; height: 9px; }
.vp-confetti.round { border-radius: 999px; }
@keyframes vp-confetti { 0% { transform: translate(0, 0) scale(0.4); opacity: 0 } 18% { opacity: 1 } 100% { transform: translate(var(--tx), var(--ty)) rotate(var(--rot)); opacity: 0 } }

/* warn / aborted */
.vp-warn { width: 74px; height: 74px; margin: 0 auto; border-radius: 999px; background: hsl(0 70% 55% / 0.13); border: 1.5px solid hsl(0 70% 55% / 0.4); display: flex; align-items: center; justify-content: center; color: #e5484d; animation: vp-pop 0.5s cubic-bezier(0.2, 1.4, 0.4, 1) both; }

/* radar */
.vp-radar { position: relative; width: 236px; height: 236px; margin: 22px auto 6px; }
.vp-radar-disk { position: absolute; inset: 0; border-radius: 999px; overflow: hidden; background: radial-gradient(circle at 50% 50%, hsl(var(--primary) / 0.1), hsl(var(--primary) / 0.02) 55%, transparent 72%); border: 1px solid hsl(var(--border)); box-shadow: inset 0 0 40px rgba(0, 0, 0, 0.3); }
.vp-ring { position: absolute; left: 50%; top: 50%; transform: translate(-50%, -50%); border-radius: 999px; border: 1px solid hsl(var(--border)); }
.vp-cross-v { position: absolute; left: 50%; top: 0; width: 1px; height: 100%; background: hsl(var(--border)); }
.vp-cross-h { position: absolute; top: 50%; left: 0; height: 1px; width: 100%; background: hsl(var(--border)); }
.vp-radar-glow { position: absolute; left: 50%; top: 50%; transform: translate(-50%, -50%); width: 200px; height: 200px; border-radius: 999px; background: radial-gradient(circle, hsl(var(--primary) / 0.18), transparent 62%); animation: vp-breathe 3.4s ease-in-out infinite; }
@keyframes vp-breathe { 0%, 100% { transform: translate(-50%, -50%) scale(0.92); opacity: 0.45 } 50% { transform: translate(-50%, -50%) scale(1.06); opacity: 0.8 } }
.vp-sweep { position: absolute; left: 50%; top: 50%; width: 236px; height: 236px; border-radius: 999px; background: conic-gradient(from 0deg, transparent 0deg, hsl(var(--primary) / 0.05) 16deg, hsl(var(--primary) / 0.3) 44deg, hsl(var(--primary) / 0.05) 72deg, transparent 88deg); animation: vp-sweep 3.4s linear infinite; }
@keyframes vp-sweep { from { transform: translate(-50%, -50%) rotate(0deg) } to { transform: translate(-50%, -50%) rotate(360deg) } }
.vp-radar-core { position: absolute; left: 50%; top: 50%; transform: translate(-50%, -50%); width: 56px; height: 56px; border-radius: 999px; background: hsl(var(--secondary)); border: 1px solid hsl(var(--primary) / 0.45); display: flex; align-items: center; justify-content: center; box-shadow: 0 0 30px hsl(var(--primary) / 0.35); z-index: 2; }
.vp-logo-dot { width: 22px; height: 22px; border-radius: 6px; background: hsl(var(--primary)); box-shadow: 0 0 14px hsl(var(--primary) / 0.7); }

/* radar list */
.vp-list { display: flex; flex-direction: column; gap: 9px; margin-top: 14px; }
.vp-empty { display: flex; align-items: center; justify-content: center; gap: 8px; padding: 16px; font-size: 13px; color: hsl(var(--muted-foreground)); }
.vp-dot-pulse { width: 8px; height: 8px; border-radius: 999px; background: hsl(var(--primary)); animation: vp-blink 1.4s ease-in-out infinite; }
@keyframes vp-blink { 0%, 100% { opacity: 0.3 } 50% { opacity: 1 } }
.vp-row { display: flex; align-items: center; gap: 12px; padding: 11px 13px; border-radius: 15px; background: hsl(var(--foreground) / 0.04); border: 1px solid hsl(var(--border)); cursor: pointer; animation: vp-deviceIn 0.5s cubic-bezier(0.2, 1, 0.3, 1) both; transition: background-color 0.28s ease, border-color 0.28s ease; }
.vp-row:hover { background: hsl(var(--foreground) / 0.08); border-color: hsl(var(--primary) / 0.4); }
@keyframes vp-deviceIn { 0% { transform: translateY(10px); opacity: 0 } 100% { transform: translateY(0); opacity: 1 } }
.vp-row-icon { width: 40px; height: 40px; border-radius: 11px; flex: none; display: flex; align-items: center; justify-content: center; background: hsl(var(--foreground) / 0.05); border: 1px solid hsl(var(--border)); color: hsl(var(--foreground)); }
.vp-bars { display: flex; align-items: flex-end; gap: 2px; height: 18px; }
.vp-bar { width: 4px; border-radius: 2px; background: hsl(var(--foreground) / 0.12); }
.vp-bar.on { background: hsl(var(--primary)); }
.vp-pill { padding: 8px 16px; border-radius: 999px; background: hsl(var(--primary)); color: hsl(var(--primary-foreground)); font-size: 12.5px; font-weight: 700; flex: none; }
</style>
