import type { StreamState } from "./events.js";

interface TextHydrationState {
  /** Full birth text was seen; accumulated deltas no longer represent a suffix-only attach. */
  baseKnown: Set<string>;
  /** Tombstones prevent an older in-flight history response from resurrecting completed/closed stream parts. */
  completed: Set<string>;
}

// Snapshot reconciliation metadata is transport-local, not active turn/renderer state.
// It must outlive turn cleanup because history responses can arrive after turn completion.
// Replacing/discarding a session StreamState releases it through this weak association.
const hydrationStates = new WeakMap<StreamState, TextHydrationState>();
export function textHydrationState(state: StreamState): TextHydrationState {
  let hydration = hydrationStates.get(state);
  if (!hydration) {
    hydration = { baseKnown: new Set(), completed: new Set() };
    hydrationStates.set(state, hydration);
  }
  return hydration;
}
