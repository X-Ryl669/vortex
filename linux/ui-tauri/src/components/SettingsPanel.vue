<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { Moon, Sun, Languages } from "lucide-vue-next";
import Modal from "./Modal.vue";
import Button from "./Button.vue";
import { theme } from "@/lib/theme";
import { setLocale, LOCALES, type LocaleCode } from "@/lib/i18n";

defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "dismiss"): void }>();
const { t, locale } = useI18n();

function pickLocale(code: LocaleCode) {
  setLocale(code);
}
function pickTheme(mode: "light" | "dark") {
  theme.value = mode;
}
</script>

<template>
  <Modal :open="open" :title="t('settings.title')" @dismiss="emit('dismiss')">
    <div class="space-y-5">
      <!-- Theme -->
      <div>
        <div class="flex items-center gap-2 mb-2 text-sm font-medium">
          <Moon class="h-4 w-4" />
          {{ t("settings.theme") }}
        </div>
        <div class="grid grid-cols-2 gap-2">
          <button
            class="flex items-center justify-center gap-2 h-9 rounded-md border border-border text-sm transition-colors"
            :class="theme === 'dark' ? 'bg-primary text-primary-foreground border-primary' : 'hover:bg-accent'"
            @click="pickTheme('dark')"
          >
            <Moon class="h-4 w-4" />
            {{ t("settings.theme_dark") }}
          </button>
          <button
            class="flex items-center justify-center gap-2 h-9 rounded-md border border-border text-sm transition-colors"
            :class="theme === 'light' ? 'bg-primary text-primary-foreground border-primary' : 'hover:bg-accent'"
            @click="pickTheme('light')"
          >
            <Sun class="h-4 w-4" />
            {{ t("settings.theme_light") }}
          </button>
        </div>
      </div>

      <!-- Language -->
      <div>
        <div class="flex items-center gap-2 mb-2 text-sm font-medium">
          <Languages class="h-4 w-4" />
          {{ t("settings.language") }}
        </div>
        <div class="space-y-1.5">
          <button
            v-for="l in LOCALES"
            :key="l.code"
            class="w-full flex items-center justify-between h-9 px-3 rounded-md border border-border text-sm transition-colors"
            :class="locale === l.code ? 'bg-primary/15 border-primary text-primary' : 'hover:bg-accent'"
            @click="pickLocale(l.code)"
          >
            <span>{{ l.label }}</span>
            <span v-if="locale === l.code" class="text-xs">●</span>
          </button>
        </div>
      </div>

      <div class="flex justify-end pt-2">
        <Button variant="outline" size="sm" @click="emit('dismiss')">{{ t("settings.close") }}</Button>
      </div>
    </div>
  </Modal>
</template>
