<script setup lang="ts">
// "Handoff" — a link open on the phone (left) flies over and lands centred on
// the laptop (right), then dissolves into the screen as the browser window
// opens with the same page. The laptop screen starts EMPTY and only fills in
// once the card has arrived, so the continuity reads as "it moved here".
</script>

<template>
  <div class="stage">
    <!-- phone with an open link -->
    <div class="phone">
      <div class="pscreen">
        <div class="urlbar"><span class="globe" /><span class="u" /></div>
        <span class="row w80" /><span class="row w60" /><span class="row w70" />
      </div>
    </div>

    <!-- the page handing off -->
    <div class="card">
      <span class="globe sm" /><span class="u sm" />
    </div>

    <!-- chevrons phone -> laptop -->
    <div class="link">
      <span class="chev" style="animation-delay:0s" />
      <span class="chev" style="animation-delay:.4s" />
    </div>

    <!-- laptop opening the same page (empty until the card lands) -->
    <div class="laptop">
      <div class="lid">
        <div class="lwin">
          <div class="urlbar"><span class="globe" /><span class="u long" /></div>
          <div class="page">
            <span class="row w90" /><span class="row w70" /><span class="row w50" />
          </div>
        </div>
      </div>
      <div class="base" />
    </div>
  </div>
</template>

<style scoped>
.stage { position: relative; width: 300px; height: 132px; display: inline-flex; align-items: center; justify-content: center; animation: vxo-featIn 0.45s cubic-bezier(0.34, 1.3, 0.5, 1) both; }

/* shared bits */
.globe { width: 9px; height: 9px; border-radius: 50%; border: 1.6px solid #2ECC71; flex: none; }
.globe.sm { width: 7px; height: 7px; border-width: 1.4px; }
.u { height: 4px; border-radius: 2px; background: rgba(255, 255, 255, 0.22); flex: 1; }
.u.sm { width: 20px; flex: none; }
.u.long { max-width: 70px; }
.urlbar { display: flex; align-items: center; gap: 5px; width: 100%; }
.row { display: block; height: 4px; border-radius: 2px; background: rgba(255, 255, 255, 0.16); }
.row.w90 { width: 90%; } .row.w80 { width: 80%; } .row.w70 { width: 70%; } .row.w60 { width: 60%; } .row.w50 { width: 50%; }

/* phone */
.phone { width: 56px; height: 96px; border-radius: 14px; background: rgba(255, 255, 255, 0.05); border: 1px solid rgba(255, 255, 255, 0.1); display: flex; align-items: center; justify-content: center; flex: none; }
.pscreen { width: 42px; height: 80px; border-radius: 10px; background: #14241b; display: flex; flex-direction: column; align-items: flex-start; gap: 8px; padding: 10px 8px; }
.pscreen .row { width: 80%; }

/* flying page card — leaves the phone, lands centred on the laptop, holds, then
   dissolves into the screen as the window opens. */
.card { position: absolute; left: 56px; top: 50%; z-index: 4; display: flex; align-items: center; gap: 4px; padding: 5px 7px; border-radius: 7px; background: #1b3325; border: 1px solid rgba(46, 204, 113, 0.5); box-shadow: 0 6px 16px rgba(46, 204, 113, 0.35); animation: pageFly 3.2s ease-in-out infinite; }
@keyframes pageFly {
  0%, 8%   { transform: translate(0, -50%) scale(0.6); opacity: 0; }
  18%      { transform: translate(0, -50%) scale(1); opacity: 1; }
  48%      { transform: translate(115px, -50%) scale(1); opacity: 1; }   /* arrived at laptop centre */
  58%      { transform: translate(115px, -50%) scale(1); opacity: 1; }   /* hold */
  68%      { transform: translate(115px, -50%) scale(0.7); opacity: 0; } /* dissolve into the screen */
  100%     { transform: translate(115px, -50%) scale(0.7); opacity: 0; }
}

/* chevrons — flow only while the card is travelling */
.link { width: 38px; display: flex; align-items: center; justify-content: center; gap: 5px; flex: none; }
.chev { width: 8px; height: 8px; border-top: 2px solid #2ECC71; border-right: 2px solid #2ECC71; transform: rotate(45deg); opacity: 0; animation: linkFlow 3.2s ease-in-out infinite; }
@keyframes linkFlow {
  0%, 12%   { opacity: 0; transform: rotate(45deg) translateX(-3px); }
  24%       { opacity: 1; }
  46%       { opacity: 1; }
  56%, 100% { opacity: 0; transform: rotate(45deg) translateX(3px); }
}

/* laptop — screen is EMPTY until the card lands, then the window opens */
.laptop { display: flex; flex-direction: column; align-items: center; flex: none; }
.lid { width: 118px; height: 76px; border-radius: 10px; background: #14241b; border: 1px solid rgba(255, 255, 255, 0.1); padding: 9px; display: flex; align-items: center; justify-content: center; }
.lwin { width: 100%; height: 100%; box-sizing: border-box; border-radius: 6px; background: rgba(255, 255, 255, 0.05); border: 1px solid rgba(255, 255, 255, 0.1); display: flex; flex-direction: column; gap: 6px; padding: 7px 8px; overflow: hidden; transform-origin: center; animation: winOpen 3.2s ease-in-out infinite; }
@keyframes winOpen {
  0%, 56%  { opacity: 0; transform: scale(0.94); }   /* empty screen until the card arrives */
  68%      { opacity: 1; transform: scale(1); }       /* window opens as the card dissolves in */
  86%      { opacity: 1; transform: scale(1); }       /* holds the page */
  96%, 100% { opacity: 0; transform: scale(0.98); }   /* fades out gracefully before the next cycle */
}
.page { display: flex; flex-direction: column; gap: 5px; }
.base { width: 138px; height: 6px; border-radius: 0 0 7px 7px; background: rgba(255, 255, 255, 0.12); margin-top: -1px; }
</style>
