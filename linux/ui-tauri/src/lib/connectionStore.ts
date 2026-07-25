import { ref } from "vue";
import {
  onIdentity,
  onPeers,
  onPeerState,
  getPeerStates,
  type IdentityInfo,
  type TrustedPeer,
  type PeerState,
} from "@/lib/bridge";

// App-lifetime connection state. Lives OUTSIDE any page component so it
// survives route changes — navigating to /settings and back must not blank the
// peer list / flip the laptop card to "offline" while the first heartbeat after
// remount is still in flight. Pages import these refs read-only; the daemon
// events below are the single writer.
export const identity = ref<IdentityInfo | null>(null);
export const peers = ref<TrustedPeer[]>([]);
export const peersLoaded = ref(false);
export const peerStates = ref<Record<string, PeerState>>({});

let started = false;

/** Subscribe to the daemon's identity/peers/peer-state events exactly once,
 *  at app start (see main.ts). Idempotent. */
export async function initConnectionStore(): Promise<void> {
  if (started) return;
  started = true;
  await onIdentity((v) => (identity.value = v));
  await onPeers((v) => {
    peers.value = v;
    peersLoaded.value = true;
  });
  await onPeerState((s) => {
    peerStates.value = { ...peerStates.value, [s.peer_static_pub]: s };
  });
  // Self-heal backstop: the pushed peer_state events above can silently stop
  // being delivered to the webview (a dead Tauri listener left the device card
  // frozen at "offline" until a manual reload re-subscribed). Poll the daemon's
  // cached per-peer state over the invoke channel — independent of the event
  // channel — so the card always recovers within one interval. The freshness
  // `ts` rides along, so a genuine disconnect still falls to "offline" via the
  // home page's clock ticker.
  const pollPeerStates = async () => {
    try {
      for (const s of await getPeerStates()) {
        peerStates.value = { ...peerStates.value, [s.peer_static_pub]: s };
      }
    } catch {
      /* daemon not ready yet / transient — the next tick retries */
    }
  };
  await pollPeerStates();
  setInterval(pollPeerStates, 15_000);
}
