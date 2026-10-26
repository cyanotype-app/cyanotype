// Class names of the form controls in the viewer's sheets (edit date, location, caption), so they read alike.

/** A text field, select or textarea. 16px text, so a phone does not zoom in when it is focused. */
export const inputClass =
  "block w-full rounded-card border border-line bg-bg px-3 py-2 text-base text-fg placeholder:text-muted " +
  "focus-visible:border-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/25 aria-[invalid=true]:border-danger";

export const labelClass = "mb-1.5 block text-sm font-medium";
