import { create } from "zustand";

/**
 * Whether the agents panel is open, and which delegation it's showing.
 *
 * Its own small store rather than state in `App`, because the request to open
 * it comes from deep inside the transcript: a delegation card's "Watch", five
 * components down. Threading a callback through each of them for one button
 * is the kind of plumbing that ends up passed to the wrong layer.
 */
export const useAgentsView = create<{
  open: boolean;
  /** The parent call id of the delegation to show opened up, if any. */
  focus: string | null;
  /** Opens the panel, on `id` when one is named. */
  show: (id?: string) => void;
  close: () => void;
}>((set) => ({
  open: false,
  focus: null,
  show: (id) => set({ open: true, focus: id ?? null }),
  close: () => set({ open: false, focus: null }),
}));
