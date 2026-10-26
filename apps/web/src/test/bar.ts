import { screen, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import "./dialogs";

/** Press an action of the selection bar: on the bar if there is room for it, else in its "More actions" menu. */
export async function pressBar(user: UserEvent, name: string | RegExp) {
  const bar = screen.getByRole("toolbar", { name: "Selection" });
  const direct = within(bar).queryByRole("button", { name });
  if (direct) return user.click(direct);
  await user.click(within(bar).getByRole("button", { name: "More actions" }));
  await user.click(await screen.findByRole("menuitem", { name }));
}

/** Choose a radio row of the page's "View options" menu. */
export async function chooseView(user: UserEvent, name: string | RegExp) {
  await user.click(screen.getByRole("button", { name: "View options" }));
  await user.click(await screen.findByRole("menuitemradio", { name }));
}

/** Whether a selection bar action is disabled, on the bar or in its "More actions" menu (which is left open). */
export async function barDisabled(user: UserEvent, name: string | RegExp): Promise<boolean> {
  const bar = screen.getByRole("toolbar", { name: "Selection" });
  let el: HTMLElement | null = within(bar).queryByRole("button", { name });
  if (!el) {
    if (!screen.queryByRole("menu")) await user.click(within(bar).getByRole("button", { name: "More actions" }));
    el = await screen.findByRole("menuitem", { name });
  }
  return el.hasAttribute("disabled") || el.getAttribute("aria-disabled") === "true";
}
