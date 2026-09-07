<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { ArrowLeft, CheckCircle2, AlertTriangle, XCircle, Copy, RefreshCw } from "lucide-vue-next";
import { invoke } from "@tauri-apps/api/core";

type Check = { id: string; level: string; detail: string };
type Diagnostics = { app_version: string; checks: Check[]; log_path: string };

const router = useRouter();
const { t, te } = useI18n();

const data = ref<Diagnostics | null>(null);
const loading = ref(true);
const copied = ref(false);

async function load() {
  loading.value = true;
  try {
    data.value = await invoke<Diagnostics>("diagnostics");
  } catch {
    data.value = null;
  }
  loading.value = false;
}

/** The point of the button: a reporter should not have to assemble anything. */
async function copyReport() {
  try {
    const text = await invoke<string>("diagnostics_report");
    await navigator.clipboard.writeText(text);
    copied.value = true;
    setTimeout(() => (copied.value = false), 2000);
  } catch {
    /* clipboard refused — nothing useful to say */
  }
}

/** Translate when we have a string for this check, else show the id. New
 *  checks then appear as themselves rather than as a blank row. */
function label(id: string): string {
  const key = `diagnostics.check.${id}`;
  return te(key) ? t(key) : id;
}

onMounted(load);
</script>

<template>
  <div class="min-h-screen flex flex-col bg-background">
    <header class="flex items-center gap-1 px-3 py-2.5 border-b border-border bg-card/40">
      <button
        class="h-9 w-9 rounded-md flex items-center justify-center hover:bg-accent transition-colors"
        @click="router.push('/settings')"
      >
        <ArrowLeft class="h-4 w-4" />
      </button>
      <h1 class="text-base font-semibold ml-1">{{ t("diagnostics.title") }}</h1>
    </header>

    <main class="flex-1 overflow-auto">
      <div class="w-full px-7 pt-8 pb-14 max-w-2xl">
        <h1 class="text-[25px] font-semibold tracking-[-0.5px]">{{ t("diagnostics.title") }}</h1>
        <p class="text-sm text-muted-foreground mt-1.5">{{ t("diagnostics.subtitle") }}</p>

        <div v-if="loading" class="mt-8 text-sm text-muted-foreground">
          {{ t("diagnostics.loading") }}
        </div>

        <template v-else-if="data">
          <div class="mt-7 rounded-lg border border-border divide-y divide-border">
            <div
              v-for="c in data.checks"
              :key="c.id"
              class="flex items-start gap-3 px-4 py-3"
            >
              <CheckCircle2 v-if="c.level === 'ok'" class="h-4 w-4 mt-0.5 shrink-0 text-emerald-500" />
              <AlertTriangle v-else-if="c.level === 'warn'" class="h-4 w-4 mt-0.5 shrink-0 text-amber-500" />
              <XCircle v-else class="h-4 w-4 mt-0.5 shrink-0 text-red-500" />
              <div class="min-w-0">
                <div class="text-sm font-medium">{{ label(c.id) }}</div>
                <div class="text-xs text-muted-foreground mt-0.5 break-words">{{ c.detail }}</div>
              </div>
            </div>
          </div>

          <div class="mt-5 flex items-center gap-2">
            <button
              class="h-9 px-3 rounded-md border border-border text-sm flex items-center gap-2 hover:bg-accent transition-colors"
              @click="copyReport"
            >
              <Copy class="h-3.5 w-3.5" />
              {{ copied ? t("diagnostics.copied") : t("diagnostics.copy") }}
            </button>
            <button
              class="h-9 px-3 rounded-md border border-border text-sm flex items-center gap-2 hover:bg-accent transition-colors"
              @click="load"
            >
              <RefreshCw class="h-3.5 w-3.5" />
              {{ t("diagnostics.refresh") }}
            </button>
          </div>

          <p class="text-xs text-muted-foreground mt-5 break-all">
            {{ t("diagnostics.logAt") }} <code>{{ data.log_path }}</code>
          </p>
          <p class="text-xs text-muted-foreground mt-1">Vortex {{ data.app_version }}</p>
        </template>
      </div>
    </main>
  </div>
</template>
