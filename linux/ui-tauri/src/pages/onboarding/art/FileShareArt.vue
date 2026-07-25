<script setup lang="ts">
// "File sharing" — a photo/file leaves the phone (left), arcs across, and lands
// in the laptop (right) where a download bar fills and a check confirms it. Reads
// as instant-share style send-a-file-across, in sync.
</script>

<template>
  <div class="stage">
    <!-- phone holding files -->
    <div class="phone">
      <div class="pscreen">
        <span class="file" /><span class="file" /><span class="file hot" />
      </div>
    </div>

    <!-- file flying across -->
    <div class="fly">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#07120D" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="4" y="3" width="16" height="18" rx="2" fill="#07120D" stroke="none" />
        <circle cx="9.5" cy="9" r="1.6" fill="#2ECC71" />
        <path d="M6 17l4-4 3 3 2-2 3 3" stroke="#2ECC71" />
      </svg>
    </div>

    <!-- chevrons phone -> laptop -->
    <div class="link">
      <span class="chev" style="animation-delay:0s" />
      <span class="chev" style="animation-delay:.4s" />
    </div>

    <!-- laptop receiving -->
    <div class="laptop">
      <div class="lid">
        <div class="tray">
          <svg class="dl" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#2ECC71" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 3v11" /><path d="M7 10l5 5 5-5" /><path d="M5 20h14" />
          </svg>
          <svg class="ok" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="#2ECC71" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 13l4 4L19 7" />
          </svg>
          <div class="bar"><span class="fill" /></div>
        </div>
      </div>
      <div class="base" />
    </div>
  </div>
</template>

<style scoped>
.stage { position: relative; width: 300px; height: 132px; display: inline-flex; align-items: center; justify-content: center; animation: vxo-featIn 0.45s cubic-bezier(0.34, 1.3, 0.5, 1) both; }

/* phone with a little file list */
.phone { width: 56px; height: 96px; border-radius: 14px; background: rgba(255, 255, 255, 0.05); border: 1px solid rgba(255, 255, 255, 0.1); display: flex; align-items: center; justify-content: center; flex: none; }
.pscreen { width: 42px; height: 80px; border-radius: 10px; background: #14241b; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px; padding: 0 9px; }
.file { display: block; width: 100%; height: 9px; border-radius: 3px; background: rgba(255, 255, 255, 0.16); }
.file.hot { background: #2ECC71; box-shadow: 0 0 12px rgba(46, 204, 113, 0.55); animation: hotPulse 2.6s ease-in-out infinite; }
@keyframes hotPulse { 0%, 14% { opacity: 1 } 22% { opacity: 0.25 } 100% { opacity: 1 } }

/* flying file */
.fly { position: absolute; left: 54px; top: 50%; z-index: 4; width: 26px; height: 26px; border-radius: 7px; background: #2ECC71; display: flex; align-items: center; justify-content: center; box-shadow: 0 6px 16px rgba(46, 204, 113, 0.45); animation: fileFly 2.6s ease-in-out infinite; }
@keyframes fileFly {
  0%, 14% { transform: translate(0, -50%) scale(0.5); opacity: 0 }
  24%     { transform: translate(6px, -50%) scale(1); opacity: 1 }
  50%     { transform: translate(128px, -50%) scale(1); opacity: 1 }   /* arrived at laptop centre */
  60%     { transform: translate(128px, -50%) scale(1); opacity: 1 }   /* hold */
  70%     { transform: translate(128px, -50%) scale(0.5); opacity: 0 } /* dissolves into the tray */
  100%    { transform: translate(128px, -50%) scale(0.5); opacity: 0 }
}

/* chevrons */
.link { width: 38px; display: flex; align-items: center; justify-content: center; gap: 5px; flex: none; }
.chev { width: 8px; height: 8px; border-top: 2px solid #2ECC71; border-right: 2px solid #2ECC71; transform: rotate(45deg); opacity: 0; animation: linkFlow 2.6s ease-in-out infinite; }
@keyframes linkFlow { 0%, 18% { opacity: 0; transform: rotate(45deg) translateX(-3px) } 34% { opacity: 1 } 50% { opacity: 1 } 64%, 100% { opacity: 0; transform: rotate(45deg) translateX(3px) } }

/* laptop receiving */
.laptop { display: flex; flex-direction: column; align-items: center; flex: none; }
.lid { width: 116px; height: 74px; border-radius: 10px; background: #14241b; border: 1px solid rgba(255, 255, 255, 0.1); display: flex; align-items: center; justify-content: center; }
.tray { position: relative; width: 64px; display: flex; flex-direction: column; align-items: center; gap: 9px; }
.dl { animation: dlShow 2.6s ease-in-out infinite; }
@keyframes dlShow { 0%, 58% { opacity: 1 } 64%, 100% { opacity: 0 } }
.ok { position: absolute; top: -1px; animation: okShow 2.6s ease-in-out infinite; }
@keyframes okShow { 0%, 60% { opacity: 0; transform: scale(0.4) } 70% { opacity: 1; transform: scale(1.15) } 78%, 100% { opacity: 1; transform: scale(1) } }
.bar { width: 100%; height: 4px; border-radius: 2px; background: rgba(255, 255, 255, 0.14); overflow: hidden; }
.fill { display: block; height: 100%; border-radius: 2px; background: #2ECC71; animation: barFill 2.6s ease-in-out infinite; }
@keyframes barFill { 0%, 24% { width: 0 } 56% { width: 100% } 100% { width: 100% } }
.base { width: 136px; height: 6px; border-radius: 0 0 7px 7px; background: rgba(255, 255, 255, 0.12); margin-top: -1px; }
</style>
