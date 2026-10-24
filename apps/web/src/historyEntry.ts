// One history entry that a piece of UI pushes while it is open (marked in history.state) and pops when it closes.
// The pop is asynchronous: history.back() only changes history.state once the browser fires popstate. Until then the
// marker is still on the current entry, so a second close (Escape twice quickly, or the panel's close followed by the
// viewer's) would call back() again and step past the entry we pushed, out of the app. `pending` remembers that our
// pop is already on its way, and later callers wait for it instead of issuing their own.

export interface HistoryEntry {
  /** True while our entry is the current one and no pop of it is in flight. */
  has(): boolean;
  /** Push the entry unless it is already current. */
  push(): void;
  /** Pop the entry if (and only if) it is current. Resolves once history has settled; never goes back further. */
  pop(): Promise<void>;
}

export function createHistoryEntry(marker: string, win: Window = window): HistoryEntry {
  let pending: Promise<void> | null = null;
  const marked = () => (win.history.state as Record<string, unknown> | null)?.[marker] === true;

  return {
    has: () => !pending && marked(),
    push() {
      if (pending || marked()) return;
      win.history.pushState({ ...(win.history.state as object | null), [marker]: true }, "");
    },
    pop() {
      if (pending) return pending;
      if (!marked()) return Promise.resolve();
      pending = new Promise<void>((done) => {
        const finish = () => {
          win.removeEventListener("popstate", finish);
          clearTimeout(timer);
          pending = null;
          done();
        };
        // If the browser never reports the pop, do not stay stuck.
        const timer = setTimeout(finish, 1000);
        win.addEventListener("popstate", finish);
        win.history.back();
      });
      return pending;
    },
  };
}
