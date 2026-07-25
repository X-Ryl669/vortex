<script setup lang="ts">
import { useI18n } from "vue-i18n";
import Modal from "@/components/Modal.vue";
import Button from "@/components/Button.vue";
import type { TrustedPeer } from "@/lib/bridge";

defineProps<{ target: TrustedPeer | null }>();
const emit = defineEmits<{ dismiss: []; confirm: [] }>();
const { t } = useI18n();
</script>

<template>
  <Modal :open="target !== null" :title="t('peers.forget_title')" @dismiss="emit('dismiss')">
    <div v-if="target" class="space-y-4">
      <p class="text-sm text-muted-foreground">
        {{ t("peers.forget_body", { name: target.peer_name || t("device.android") }) }}
      </p>
      <div class="flex gap-2 justify-end">
        <Button variant="ghost" size="sm" @click="emit('dismiss')">
          {{ t("scan.close") }}
        </Button>
        <Button variant="destructive" size="sm" @click="emit('confirm')">
          {{ t("peers.forget_confirm") }}
        </Button>
      </div>
    </div>
  </Modal>
</template>
