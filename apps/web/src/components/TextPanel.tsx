import { useEffect, useRef, useState } from "react";
import type { PhotoText } from "../api";
import { copyText, linkHost, qrBoxes, lineBoxes, qrKind, safeLink, textStatus } from "../textText";

export type TextState = { status: "loading" } | { status: "error" } | { status: "ready"; text: PhotoText };

interface Props {
  panelId: string;
  state: TextState;
  onRetry: () => void;
  onClose: () => void;
  /** Draw boxes on the photo where the text is. */
  showBoxes: boolean;
  onShowBoxes: (on: boolean) => void;
}

/**
 * The text read off the open photo, to read and copy, and the QR codes in it. A QR code is shown as what it says, and a
 * web address as a link the person has to click: nothing is ever opened for them.
 */
export function TextPanel({ panelId, state, onRetry, onClose, showBoxes, onShowBoxes }: Props) {
  const closeRef = useRef<HTMLButtonElement>(null);
  const [note, setNote] = useState<string | null>(null);
  useEffect(() => {
    closeRef.current?.focus();
  }, []);
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
    <aside className="details-panel text-panel" id={panelId} aria-label="Text in this photo">
      <div className="details-head">
        <span className="details-grip" aria-hidden="true" />
        <h2>Text</h2>
        <button type="button" ref={closeRef} className="btn btn-glass" onClick={onClose}>
          Close
        </button>
      </div>
      <div className="details-body">
        {state.status === "loading" && (
          <p className="muted" role="status">
            Loading…
          </p>
        )}
        {state.status === "error" && (
          <p role="alert">
            Could not load the text.{" "}
            <button type="button" className="btn btn-glass" onClick={onRetry}>
              Try again
            </button>
          </p>
        )}
        {ready && textStatus(ready) && <p className="muted">{textStatus(ready)}</p>}
        {ready && ready.text.trim() !== "" && (
          <>
            <div className="text-actions">
              <button type="button" className="btn btn-glass" onClick={() => void copy(copyText(ready))}>
                Copy all
              </button>
              <label className="check">
                <input type="checkbox" checked={showBoxes} onChange={(e) => onShowBoxes(e.target.checked)} /> Show on the photo
              </label>
            </div>
            <div role="status" className="muted small">
              {note}
            </div>
            <pre className="text-body" tabIndex={0} aria-label="Recognised text">
              {ready.text}
            </pre>
            <p className="muted small">Read automatically: check numbers and names against the photo.</p>
          </>
        )}
        {ready && ready.qr.length > 0 && (
          <section aria-label="QR codes">
            <h3>QR {ready.qr.length === 1 ? "code" : "codes"}</h3>
            <ul className="qr-list">
              {ready.qr.map((c, i) => {
                const href = safeLink(c.t);
                return (
                  <li key={`${i}-${c.t}`}>
                    <span className="muted small">{qrKind(c.t)}</span>
                    <code className="qr-payload">{c.t}</code>
                    <div className="text-actions">
                      <button type="button" className="btn btn-glass" onClick={() => void copy(c.t)}>
                        Copy
                      </button>
                      {href && (
                        <a className="btn btn-glass" href={href} target="_blank" rel="noopener noreferrer nofollow">
                          Open {linkHost(href)}
                        </a>
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
            <p className="muted small">QR codes are never opened for you. Check the address before you open it.</p>
          </section>
        )}
      </div>
    </aside>
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
