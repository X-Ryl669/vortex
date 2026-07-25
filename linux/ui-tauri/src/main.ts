import { createApp } from "vue";
import "./style.css";
import App from "./App.vue";
import { i18n } from "@/lib/i18n";
import { router } from "@/router";
import { initConnectionStore } from "@/lib/connectionStore";
import { initHome } from "@/composables/useHome";
import { initContacts } from "@/composables/useContacts";
import { initRecents } from "@/composables/useRecents";
import { initMessages } from "@/composables/useMessages";
import "@/lib/theme"; // initialise theme (side-effect)

// Subscribe to peer/identity state + home logic once, app-wide, so it persists
// across route changes (no "offline" flash / no re-subscribe on remount).
initConnectionStore();
initHome();
initContacts();
initRecents();
initMessages();

createApp(App).use(i18n).use(router).mount("#app");
