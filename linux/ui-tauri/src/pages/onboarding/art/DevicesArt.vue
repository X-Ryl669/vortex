<script setup lang="ts">
// "What is Vortex" hero — a laptop + phone glowing under a soft halo, both
// carrying the real Vortex logo (same asset as the sidebar). Floats gently.
import logo from "@/assets/vortex_logo.png";
</script>

<template>
  <div style="display:flex;justify-content:center;animation:vxo-floaty 5s ease-in-out infinite;">
    <svg width="340" height="190" viewBox="0 0 340 190" fill="none" style="display:block;overflow:visible;">
      <defs>
        <radialGradient id="vxoHalo" cx="50%" cy="50%" r="50%">
          <stop offset="0%" stop-color="#2ECC71" stop-opacity=".22" />
          <stop offset="70%" stop-color="#2ECC71" stop-opacity="0" />
        </radialGradient>
      </defs>
      <ellipse cx="170" cy="98" rx="152" ry="86" fill="url(#vxoHalo)" />
      <!-- laptop -->
      <rect x="44" y="58" width="104" height="66" rx="9" fill="#141519" stroke="#34373e" stroke-width="2" />
      <rect x="50" y="64" width="92" height="54" rx="5" fill="#2ECC71" fill-opacity=".06" />
      <rect x="34" y="124" width="124" height="7" rx="3.5" fill="#24272e" />
      <image
        :href="logo"
        x="84" y="79" width="24" height="24"
        style="animation:vxo-logoPulse 3.4s ease-in-out infinite;"
      />
      <!-- phone -->
      <rect x="226" y="46" width="52" height="86" rx="12" fill="#141519" stroke="#34373e" stroke-width="2" />
      <rect x="231" y="51" width="42" height="76" rx="8" fill="#2ECC71" fill-opacity=".06" />
      <rect x="244" y="121" width="16" height="3" rx="1.5" fill="#34373e" />
      <image
        :href="logo"
        x="241" y="78" width="22" height="22"
        style="animation:vxo-logoPulse 3.4s ease-in-out infinite;"
      />
    </svg>
  </div>
</template>
