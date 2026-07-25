<script setup lang="ts">
// Earbud hops from phone to laptop while the audio EQ lights up on whichever
// device is "playing". Ported from the design's feature-0 stage.
const bars = [0, 1, 2, 3];
</script>

<template>
  <div class="vxo-stage">
    <div class="vxo-row">
      <!-- phone -->
      <div class="vxo-tile">
        <div class="vxo-tile-ic">
          <svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="#C9CCD2" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <rect x="6" y="2" width="12" height="20" rx="3" /><path d="M10.5 5h3" /><path d="M12 18h.01" />
          </svg>
        </div>
        <div class="vxo-eq" style="animation:vxo-eqPhoneOn 3.6s ease-in-out infinite;">
          <span v-for="i in bars" :key="i" :style="{ animation: `vxo-eqb ${0.55 + i * 0.12}s ease-in-out ${i * 0.07}s infinite alternate` }" />
        </div>
      </div>
      <!-- laptop -->
      <div class="vxo-tile">
        <div class="vxo-tile-ic">
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#C9CCD2" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <path d="M20 16V7a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v9" /><path d="M2 20h20l-1.3-2.55a1 1 0 0 0-.9-.55H4.2a1 1 0 0 0-.9.55z" />
          </svg>
        </div>
        <div class="vxo-eq" style="animation:vxo-eqLapOn 3.6s ease-in-out infinite;">
          <span v-for="i in bars" :key="i" :style="{ animation: `vxo-eqb ${0.55 + i * 0.12}s ease-in-out ${i * 0.07}s infinite alternate` }" />
        </div>
      </div>
    </div>
    <!-- the hopping earbud -->
    <div class="vxo-bud" style="animation:vxo-hopBud 3.6s ease-in-out infinite;">
      <span class="vxo-sonar" />
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#07120D" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 18 0v7a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3" />
      </svg>
    </div>
  </div>
</template>

<style scoped>
.vxo-stage { position: relative; width: 300px; height: 132px; display: inline-flex; align-items: center; justify-content: center; animation: vxo-featIn 0.45s cubic-bezier(0.34, 1.3, 0.5, 1) both; }
.vxo-row { display: flex; align-items: center; justify-content: space-between; width: 232px; }
.vxo-tile { width: 72px; height: 72px; border-radius: 18px; background: rgba(255, 255, 255, 0.05); border: 1px solid rgba(255, 255, 255, 0.08); position: relative; flex: none; }
.vxo-tile-ic { position: absolute; top: 11px; left: 0; right: 0; display: flex; justify-content: center; }
.vxo-eq { position: absolute; left: 50%; bottom: 8px; transform: translateX(-50%); display: flex; align-items: flex-end; gap: 2.5px; height: 14px; }
.vxo-eq span { width: 3px; height: 14px; border-radius: 2px; background: #2ECC71; transform-origin: bottom; }
.vxo-bud { position: absolute; left: 52px; top: 2px; width: 38px; height: 38px; border-radius: 999px; display: flex; align-items: center; justify-content: center; background: #2ECC71; box-shadow: 0 4px 16px rgba(46, 204, 113, 0.5); z-index: 3; }
.vxo-sonar { position: absolute; inset: 0; border-radius: 999px; border: 2px solid rgba(46, 204, 113, 0.6); animation: vxo-sonar 1.6s ease-out infinite; }
</style>
