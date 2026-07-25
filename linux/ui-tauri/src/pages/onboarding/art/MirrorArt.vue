<script setup lang="ts">
// "Mirror & control" — the phone's screen lives inside the laptop on the left,
// and a cursor taps a button on it (press + ripple). The real phone on the right
// lights the SAME button in sync, so it reads as "control your phone from the
// laptop", not just a passive cast.
</script>

<template>
  <div class="stage">
    <!-- laptop showing the mirrored phone + a cursor controlling it -->
    <div class="laptop">
      <div class="lid">
        <div class="mirror">
          <span class="row w70" /><span class="row w50" />
          <span class="btn" />
        </div>
        <div class="cursor">
          <span class="ripple" />
          <svg width="15" height="15" viewBox="0 0 24 24" fill="#F2F4F6" stroke="#0E0E10" stroke-width="1.4"><path d="M5 3l14 8-6 1.5L9.5 19z" /></svg>
        </div>
      </div>
      <div class="base" />
    </div>

    <!-- control flowing to the phone -->
    <div class="link">
      <span class="chev" style="animation-delay:0s" />
      <span class="chev" style="animation-delay:.35s" />
    </div>

    <!-- the real phone reacting in sync -->
    <div class="phone">
      <div class="pscreen">
        <span class="row w70" /><span class="row w50" />
        <span class="btn" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.stage { position: relative; width: 300px; height: 132px; display: inline-flex; align-items: center; justify-content: center; animation: vxo-featIn 0.45s cubic-bezier(0.34, 1.3, 0.5, 1) both; }

/* shared mirrored content (UI rows + the tappable button) */
.row { display: block; height: 4px; border-radius: 2px; background: rgba(255, 255, 255, 0.18); }
.row.w70 { width: 70%; } .row.w50 { width: 50%; }
.btn { display: block; height: 11px; width: 60%; border-radius: 6px; background: #2ECC71; box-shadow: 0 0 0 rgba(46, 204, 113, 0); animation: btnTap 2.4s ease-in-out infinite; }
@keyframes btnTap { 0%, 46% { background: #1d7a47; box-shadow: 0 0 0 rgba(46, 204, 113, 0) } 56%, 74% { background: #2ECC71; box-shadow: 0 0 16px rgba(46, 204, 113, 0.7) } 100% { background: #1d7a47 } }

/* laptop */
.laptop { display: flex; flex-direction: column; align-items: center; flex: none; }
.lid { position: relative; width: 116px; height: 74px; border-radius: 10px; background: #14241b; border: 1px solid rgba(255, 255, 255, 0.1); display: flex; align-items: center; justify-content: center; overflow: hidden; }
.mirror { width: 46px; height: 60px; border-radius: 7px; background: rgba(255, 255, 255, 0.05); border: 1px solid rgba(255, 255, 255, 0.12); display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 7px; padding: 0 8px; }
.base { width: 136px; height: 6px; border-radius: 0 0 7px 7px; background: rgba(255, 255, 255, 0.12); margin-top: -1px; }

/* cursor that presses the button */
/* Centred on the button: the mirror (and its centred button) sit at the lid's
   middle, ~11px below centre; offset so the pointer TIP lands on the button. */
.cursor { position: absolute; left: 50%; top: 50%; margin-left: -3px; margin-top: 8px; filter: drop-shadow(0 1px 2px rgba(0, 0, 0, 0.5)); animation: curPress 2.4s ease-in-out infinite; }
@keyframes curPress { 0%, 44% { transform: translate(0, 0) } 54% { transform: translate(-1px, 2px) scale(0.9) } 64%, 100% { transform: translate(0, 0) } }
.ripple { position: absolute; left: 2px; top: 2px; width: 10px; height: 10px; border-radius: 50%; border: 1.5px solid #2ECC71; transform: scale(0); opacity: 0; animation: curRipple 2.4s ease-in-out infinite; }
@keyframes curRipple { 0%, 48% { transform: scale(0); opacity: 0 } 56% { transform: scale(1); opacity: 0.7 } 82%, 100% { transform: scale(2.6); opacity: 0 } }

/* control link */
.link { width: 40px; display: flex; align-items: center; justify-content: center; gap: 5px; flex: none; }
.chev { width: 8px; height: 8px; border-top: 2px solid #2ECC71; border-right: 2px solid #2ECC71; transform: rotate(45deg); opacity: 0; animation: linkFlow 2.4s ease-in-out infinite; }
@keyframes linkFlow { 0%, 50% { opacity: 0; transform: rotate(45deg) translateX(-3px) } 60% { opacity: 1 } 78% { opacity: 1 } 100% { opacity: 0; transform: rotate(45deg) translateX(3px) } }

/* phone */
.phone { width: 56px; height: 96px; border-radius: 14px; background: rgba(255, 255, 255, 0.05); border: 1px solid rgba(255, 255, 255, 0.1); display: flex; align-items: center; justify-content: center; flex: none; }
.pscreen { width: 42px; height: 80px; border-radius: 10px; background: #14241b; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 9px; padding: 0 9px; }
</style>
