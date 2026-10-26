import { useState } from "react";
import type { PhotoText } from "../api";
import { copyText, linkHost, qrBoxes, lineBoxes, qrKind, safeLink, textStatus } from "../textText";
import { Button } from "../ui/Button";
import { Switch } from "../ui/Controls";
import { ViewerPanel } from "./ViewerPanel";
import "../viewer.css";

export type TextState = { status: "loading" } | { status: "error" } | { status: "ready"; text: PhotoText };

interface Props {
  panelId: string;
  state: TextState;
  onRetry: () => void;
  onClose: () => void;
  /** Draw boxes on the photo where the text is. */
  showBoxes: boolean;
  onShowBoxes: (on: boolean) => void;
  /** Whether the panel is shown (the viewer keeps it mounted while it animates away). */
  open?: boolean;
}

/**
 * The text read off the open photo, to read and copy, and the QR codes in it. A QR code is shown as what it says, and a
 * web address as a link the person has to click: nothing is ever opened for them.
 */
export function TextPanel({ panelId, state, onRetry, onClose, showBoxes, onShowBoxes, open = true }: Props) {
  const [note, setNote] = useState<string | null>(null);
  const ready = state.status === "ready" ? state.text : null;

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setNote("Copied.");
    } catch {
      setNote("Could not copy automatically. Select the text and copy it by hand.");
    }
  }

  return (
    <ViewerPanel id={panelId} title="Text" label="Text in this photo" closeLabel="Close" onClose={onClose} open={open}>
      <div className="flex flex-col gap-4">
        {state.status === "loading" && (
          <p className="py-4 text-muted" role="status">
            Loading…
          </p>
        )}
        {state.status === "error" && (
          <div role="alert" className="flex flex-col items-start gap-3 py-4">
            <p>Could not load the text.</p>
            <Button variant="secondary" onClick={onRetry}>
              Try again
            </Button>
          </div>
        )}
        {ready && textStatus(ready) && <p className="text-muted">{textStatus(ready)}</p>}
        {ready && ready.text.trim() !== "" && (
          <>
            <div className="flex flex-col gap-1">
              <Button variant="secondary" className="self-start" onClick={() => void copy(copyText(ready))}>
                Copy all
              </Button>
              <Switch checked={showBoxes} onChange={onShowBoxes} label="Show on the photo" />
              <div role="status" className="min-h-5 text-sm text-muted">
                {note}
              </div>
            </div>
            <pre
              className="m-0 rounded-card bg-surface p-4 font-[inherit] text-[0.9375rem] leading-relaxed break-words whitespace-pre-wrap select-text"
              tabIndex={0}
              aria-label="Recognised text"
            >
              {ready.text}
            </pre>
            <p className="text-sm text-muted">Read automatically: check numbers and names against the photo.</p>
          </>
        )}
        {ready && ready.qr.length > 0 && (
          <section aria-label="QR codes" className="flex flex-col gap-2">
            <h3 className="text-[1.0625rem] font-semibold tracking-tight">QR {ready.qr.length === 1 ? "code" : "codes"}</h3>
            <ul className="m-0 flex list-none flex-col gap-2 p-0">
              {ready.qr.map((c, i) => {
                const href = safeLink(c.t);
                return (
                  <li key={`${i}-${c.t}`} className="flex flex-col gap-2 rounded-card bg-surface p-4">
                    <span className="text-sm text-muted">{qrKind(c.t)}</span>
                    <code className="font-mono text-sm [overflow-wrap:anywhere]">{c.t}</code>
                    <div className="flex flex-wrap gap-2">
                      <Button variant="secondary" size="sm" onClick={() => void copy(c.t)}>
                        Copy
                      </Button>
                      {href && (
                        <Button asChild variant="secondary" size="sm">
                          <a href={href} target="_blank" rel="noopener noreferrer nofollow">
                            Open {linkHost(href)}
                          </a>
                        </Button>
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
            <p className="text-sm text-muted">QR codes are never opened for you. Check the address before you open it.</p>
          </section>
        )}
      </div>
    </ViewerPanel>
  );
}

/** Boxes over the displayed photo: lines of text and QR codes, as percentages of the picture (so they follow any zoom or size). */
export function TextBoxes({ text, style }: { text: PhotoText; style: { left: number; top: number; width: number; height: number } }) {
  const lines = lineBoxes(text.lines);
  const codes = qrBoxes(text.qr);
  return (
    <div
      className="text-boxes"
      aria-hidden="true"
      style={{ left: style.left, top: style.top, width: style.width, height: style.height }}
    >
      {lines.map((b) => (
        <span key={b.key} className="text-box" style={b.style} title={b.text} />
      ))}
      {codes.map((b) => (
        <span key={b.key} className="text-box qr" style={b.style} />
      ))}
    </div>
  );
}
