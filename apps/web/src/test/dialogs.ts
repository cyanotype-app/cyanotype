// jsdom lacks what the app's Sheet (`<dialog>.showModal()`) and its menus (Radix: pointer capture, ResizeObserver, scrolling into
// view) lean on. Import this for a test that renders one.
const proto = HTMLDialogElement.prototype as unknown as Record<string, unknown>;
if (typeof proto.showModal !== "function") {
  proto.showModal = function (this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  proto.close = function (this: HTMLDialogElement) {
    this.removeAttribute("open");
    this.dispatchEvent(new Event("close"));
  };
}
const el = Element.prototype as unknown as Record<string, unknown>;
el.hasPointerCapture ??= () => false;
el.setPointerCapture ??= () => undefined;
el.releasePointerCapture ??= () => undefined;
el.scrollIntoView ??= () => undefined;
if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}
export {};
