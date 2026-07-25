<script setup lang="ts">
// The phone drifts toward/away from the laptop, whose lock badge flips
// closed → open — "proximity lock". Ported from the design's feature-4 stage.
</script>

<template>
  <div class="vxo-stage">
    <!-- the phone moving in and out of range -->
    <div class="vxo-mover" style="animation:vxo-proxMove 2.8s ease-in-out infinite;">
      <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#C9CCD2" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <rect x="6" y="2" width="12" height="20" rx="3" /><path d="M10.5 5h3" /><path d="M12 18h.01" />
      </svg>
    </div>
    <!-- laptop with its lock badge -->
    <div class="vxo-tile">
      <svg width="30" height="30" viewBox="0 0 24 24" fill="none" stroke="#C9CCD2" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <path d="M20 16V7a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v9" /><path d="M2 20h20l-1.3-2.55a1 1 0 0 0-.9-.55H4.2a1 1 0 0 0-.9.55z" />
      </svg>
      <div class="vxo-lock">
        <span class="vxo-lockface" style="animation:vxo-lockClosedK 2.8s ease-in-out infinite;">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#9A9DA3" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 11h14a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1z" /><path d="M8 11V7a4 4 0 0 1 8 0v4" />
          </svg>
        </span>
        <span class="vxo-lockface" style="animation:vxo-lockOpenK 2.8s ease-in-out infinite;">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#2ECC71" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 11h14a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1z" /><path d="M8 11V7a4 4 0 0 1 7.6-1.6" />
          </svg>
        </span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.vxo-stage { position: relative; width: 300px; height: 132px; display: inline-flex; align-items: center; justify-content: center; animation: vxo-featIn 0.45s cubic-bezier(0.34, 1.3, 0.5, 1) both; }
.vxo-mover { position: absolute; left: 78px; top: 50px; z-index: 3; }
.vxo-tile { width: 72px; height: 72px; border-radius: 18px; display: flex; align-items: center; justify-content: center; background: rgba(255, 255, 255, 0.05); border: 1px solid rgba(255, 255, 255, 0.08); position: relative; flex: none; margin-left: 56px; }
.vxo-lock { position: absolute; right: -9px; top: -9px; width: 28px; height: 28px; border-radius: 999px; background: #17181D; border: 1px solid rgba(255, 255, 255, 0.12); }
.vxo-lockface { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; }
</style>
