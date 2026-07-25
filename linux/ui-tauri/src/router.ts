import { createRouter, createWebHashHistory } from "vue-router";

// Hash history: a Tauri app loads from a file:// / custom-scheme origin with no
// server to handle deep links, so hash routing avoids 404s on reload.
// Pages live under src/pages/<name>/. Lazy-import keeps each page in its own
// chunk so the app scales as more pages are added.
export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", name: "home", component: () => import("@/pages/home/HomePage.vue") },
    { path: "/contacts", name: "contacts", component: () => import("@/pages/contacts/ContactsPage.vue") },
    { path: "/recents", name: "recents", component: () => import("@/pages/recents/RecentsPage.vue") },
    { path: "/messages", name: "messages", component: () => import("@/pages/messages/MessagesPage.vue") },
    { path: "/notes", name: "notes", component: () => import("@/pages/notes/NotesPage.vue") },
    // Open thread lives in the URL so history (and the mouse back button)
    // closes the chat instead of leaving the Messages page.
    {
      path: "/messages/:address",
      name: "messages-thread",
      component: () => import("@/pages/messages/MessagesPage.vue"),
    },
    { path: "/settings", name: "settings", component: () => import("@/pages/settings/SettingsPage.vue") },
    // Standalone popup window (no AppShell rail) — see App.vue.
    { path: "/clipboard", name: "clipboard", component: () => import("@/pages/clipboard/ClipboardPage.vue") },
  ],
});
