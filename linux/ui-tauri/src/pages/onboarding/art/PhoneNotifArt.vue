<script setup lang="ts">
// Phone + laptop with a notification card sliding in over the laptop —
// "your phone, on your laptop". Ported from the design's feature-1 stage.
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
    <div class="vxo-notif" style="animation:vxo-notifSlide 3.6s ease-in-out infinite;">
      <span class="vxo-notif-ic">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="#2ECC71" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" /><path d="M9 21a3 3 0 0 0 6 0" />
        </svg>
      </span>
      <span>
        <span class="vxo-line" style="width:48px" />
        <span class="vxo-line" style="width:32px;margin-top:5px;opacity:.5" />
      </span>
    </div>
  </div>
</template>

<style scoped>
.vxo-stage { position: relative; width: 300px; height: 132px; display: inline-flex; align-items: center; justify-content: center; animation: vxo-featIn 0.45s cubic-bezier(0.34, 1.3, 0.5, 1) both; }
.vxo-row { display: flex; align-items: center; justify-content: space-between; width: 232px; }
.vxo-tile { width: 72px; height: 72px; border-radius: 18px; display: flex; align-items: center; justify-content: center; background: rgba(255, 255, 255, 0.05); border: 1px solid rgba(255, 255, 255, 0.08); flex: none; }
.vxo-notif { position: absolute; top: 20px; right: 34px; display: flex; align-items: center; gap: 8px; padding: 7px 10px; border-radius: 11px; background: #1C1D22; border: 1px solid rgba(255, 255, 255, 0.1); box-shadow: 0 8px 22px rgba(0, 0, 0, 0.5); z-index: 3; }
.vxo-notif-ic { width: 20px; height: 20px; border-radius: 6px; background: rgba(46, 204, 113, 0.18); display: flex; align-items: center; justify-content: center; flex: none; }
.vxo-line { display: block; height: 4px; border-radius: 2px; background: rgba(255, 255, 255, 0.28); }
</style>
