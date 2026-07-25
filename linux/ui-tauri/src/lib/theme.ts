import { ref, watchEffect } from "vue";

export type Theme = "light" | "dark";

const STORAGE_KEY = "vortex.theme";

function detect(): Theme {
  const stored = localStorage.getItem(STORAGE_KEY) as Theme | null;
  if (stored === "light" || stored === "dark") return stored;
  // default to dark
  return "dark";
}

export const theme = ref<Theme>(detect());

watchEffect(() => {
  const root = document.documentElement;
  if (theme.value === "dark") root.classList.add("dark");
  else root.classList.remove("dark");
  localStorage.setItem(STORAGE_KEY, theme.value);
});

export function toggleTheme() {
  theme.value = theme.value === "dark" ? "light" : "dark";
}
