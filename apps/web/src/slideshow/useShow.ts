import { useSyncExternalStore } from "react";
import type { ShowState, SlideshowController } from "./controller";

const INITIAL: ShowState = {
  status: "loading",
  slide: null,
  outgoing: null,
  position: 0,
  total: null,
  trouble: false,
  announce: "",
  fade: false,
};
const never = () => () => undefined;

/** The state of a controller (or "loading" while there is none yet). */
export function useShowState(ctl: SlideshowController | null): ShowState {
  return useSyncExternalStore(ctl ? ctl.subscribe : never, ctl ? ctl.getState : () => INITIAL);
}
