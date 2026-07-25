<script setup lang="ts">
// A copied chip flies from phone to laptop, flipping clipboard → check —
// "one clipboard". Ported from the design's feature-2 stage.
</script>

<template>
  <div class="vxo-stage">
    <div class="vxo-row">
      <div class="vxo-tile">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#C9CCD2" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <rect x="6" y="2" width="12" height="20" rx="3" /><path d="M10.5 5h3" /><path d="M12 18h.01" />
        </svg>
      </div>
      <div class="vxo-tile">
        <svg width="30" height="30" viewBox="0 0 24 24" fill="none" stroke="#C9CCD2" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M20 16V7a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v9" /><path d="M2 20h20l-1.3-2.55a1 1 0 0 0-.9-.55H4.2a1 1 0 0 0-.9.55z" />
        </svg>
      </div>
    </div>
    <div class="vxo-fly" style="animation:vxo-copyFly 3.4s ease-in-out infinite;">
      <span class="vxo-layer" style="animation:vxo-clipShow 3.4s ease-in-out infinite;">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="#07120D" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M9 4h6a1 1 0 0 1 1 1v1a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z" />
          <path d="M16 5h2a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2h2" />
        </svg>
      </span>
      <span class="vxo-layer" style="animation:vxo-checkShow 3.4s ease-in-out infinite;">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#07120D" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round">
          <path d="M20 6 9 17l-5-5" />
        </svg>
      </span>
    </div>
  </div>
</template>

<style scoped>
.vxo-stage { position: relative; width: 300px; height: 132px; display: inline-flex; align-items: center; justify-content: center; animation: vxo-featIn 0.45s cubic-bezier(0.34, 1.3, 0.5, 1) both; }
.vxo-row { display: flex; align-items: center; justify-content: space-between; width: 232px; }
.vxo-tile { width: 72px; height: 72px; border-radius: 18px; display: flex; align-items: center; justify-content: center; background: rgba(255, 255, 255, 0.05); border: 1px solid rgba(255, 255, 255, 0.08); flex: none; }
.vxo-fly { position: absolute; left: 55px; top: 51px; width: 30px; height: 30px; border-radius: 9px; background: #2ECC71; z-index: 3; box-shadow: 0 4px 14px rgba(46, 204, 113, 0.5); }
.vxo-layer { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; }
</style>
