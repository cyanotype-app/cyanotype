import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// jsdom has no modal <dialog>: open and close it the way a browser does (the `open` attribute, a `close` event), and let Escape
// cancel the topmost one when the key is pressed inside it, so sheets behave as they do in a real browser.
if (typeof HTMLDialogElement !== "undefined" && !HTMLDialogElement.prototype.showModal) {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
    // A browser focuses the first thing to type into or press (a sheet's own close button only shows on a desktop).
    const focusable = Array.from(this.querySelectorAll<HTMLElement>("[autofocus], input, textarea, select, button:not([aria-label='Close']), a[href]"));
    (focusable.find((el) => el.hasAttribute("autofocus")) ?? focusable[0])?.focus();
  };
  HTMLDialogElement.prototype.show = function show(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    if (!this.hasAttribute("open")) return;
    this.removeAttribute("open");
    this.dispatchEvent(new Event("close"));
  };
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    const open = Array.from(document.querySelectorAll("dialog[open]"));
    const top = open.at(-1) as HTMLDialogElement | undefined;
    if (!top || !top.contains(e.target as Node)) return;
    const cancel = new Event("cancel", { cancelable: true });
    top.dispatchEvent(cancel);
    if (!cancel.defaultPrevented) top.close();
  });
}

afterEach(() => cleanup());

// jsdom has no modal <dialog>: showModal just opens it (a Sheet is a dialog), close closes it.
if (typeof HTMLDialogElement !== "undefined" && typeof HTMLDialogElement.prototype.showModal !== "function") {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
}

// Radix measures some controls (a switch, a menu) with ResizeObserver, which jsdom lacks.
if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}
