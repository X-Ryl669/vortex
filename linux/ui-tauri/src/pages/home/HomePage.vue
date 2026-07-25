<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { ArrowLeftRight } from "lucide-vue-next";
import Button from "@/components/Button.vue";
import Modal from "@/components/Modal.vue";
import ForgetModal from "@/components/ForgetModal.vue";
import PairingOverlay from "@/components/PairingOverlay.vue";
import EarbudsPicker from "@/components/EarbudsPicker.vue";
import Devices from "@/pages/home/Devices.vue";
import {
  activeEarbuds,
  canSwitch,
  confirmForget,
  earbudsMenuOpen,
  forgetTarget,
  removeEarbuds,
  sendToPeer,
} from "@/composables/useHome";

const { t } = useI18n();
</script>

<template>
  <div class="min-h-screen">
    <Devices />

    <!-- Continuity-style pairing flow: radar scan → connecting → emoji security
         check → success / aborted. Self-contained; reads the pairing state. -->
    <PairingOverlay />

    <!-- Forget-device confirm -->
    <ForgetModal
      :target="forgetTarget"
      @dismiss="forgetTarget = null"
      @confirm="confirmForget"
    />

    <!-- Earbuds picker: magical Bluetooth-scan sheet (own component). -->
    <EarbudsPicker />

    <!-- Earbuds long-press / right-click → context menu. -->
    <Modal
      :open="earbudsMenuOpen"
      :title="activeEarbuds?.name || t('device.earbuds')"
      @dismiss="earbudsMenuOpen = false"
    >
      <div class="space-y-3">
        <button
          v-if="canSwitch && activeEarbuds?.on === 'local'"
          class="w-full text-left rounded-md border border-border bg-background hover:bg-accent px-3 py-2.5 text-sm font-medium transition-colors flex items-center gap-2"
          @click="sendToPeer"
        >
          <ArrowLeftRight class="h-4 w-4 text-primary" />
          <span>{{ t("switch.send_menu") }}</span>
        </button>
        <button
          class="w-full text-left rounded-md border border-border bg-background hover:bg-destructive/10 px-3 py-2.5 text-sm font-medium text-destructive transition-colors"
          @click="removeEarbuds"
        >
          {{ t("earbuds.remove") }}
        </button>
        <div class="flex justify-end pt-1">
          <Button variant="ghost" size="sm" @click="earbudsMenuOpen = false">
            {{ t("switch.cancel") }}
          </Button>
        </div>
      </div>
    </Modal>
  </div>
</template>
