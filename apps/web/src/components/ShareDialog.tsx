import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { ApiError } from "../api";
import { photoCount } from "../albumText";
import { shareApi, type ShareLink } from "../shareApi";
import { EXPIRY_PRESETS, copyText, expiryFromPreset, expiryText, shareUrl, type ExpiryPreset } from "../shareText";

/** What a new link shares: a whole album (followed live) or these photos (a fixed set). */
export type ShareTarget = { kind: "album"; albumId: string; albumTitle: string } | { kind: "photos"; ids: readonly string[] };

/** The most photos one link can hold (the server's bulk limit). */
export const MAX_SHARE_PHOTOS = 1000;

function useModal(onClose: () => void) {
  const ref = useRef<HTMLFormElement>(null);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("input, select")?.focus();
    return () => prev?.focus?.();
  }, []);
  // Capture so Escape never also closes the lightbox or clears a selection underneath.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onClose]);
  return ref;
}

interface FieldsProps {
  idPrefix: string;
  busy: boolean;
  titlePlaceholder: string;
  title: string;
  onTitle: (v: string) => void;
  /** The expiry control; its options differ between creating and editing. */
  expiry: ReactNode;
  password: string;
  onPassword: (v: string) => void;
  passwordLabel: string;
  passwordHint?: string;
  extra?: ReactNode;
  allowDownload: boolean;
  onAllowDownload: (v: boolean) => void;
  includeOriginals: boolean;
  onIncludeOriginals: (v: boolean) => void;
}

function Fields(p: FieldsProps) {
  return (
    <div className="share-fields">
      <label htmlFor={`${p.idPrefix}-title`}>Title (optional)</label>
      <input
        id={`${p.idPrefix}-title`}
        className="text-input"
        value={p.title}
        maxLength={200}
        placeholder={p.titlePlaceholder}
        onChange={(e) => p.onTitle(e.target.value)}
        disabled={p.busy}
      />
      {p.expiry}
      <label htmlFor={`${p.idPrefix}-password`}>{p.passwordLabel}</label>
      <input
        id={`${p.idPrefix}-password`}
        className="text-input"
        type="password"
        autoComplete="new-password"
        value={p.password}
        maxLength={200}
        onChange={(e) => p.onPassword(e.target.value)}
        disabled={p.busy}
      />
      {p.passwordHint && <p className="muted small share-hint">{p.passwordHint}</p>}
      {p.extra}
      <label className="share-check">
        <input
          type="checkbox"
          checked={p.allowDownload}
          onChange={(e) => p.onAllowDownload(e.target.checked)}
          disabled={p.busy}
        />
        <span>Allow downloads</span>
      </label>
      {p.allowDownload && (
        <>
          <label className="share-check">
            <input
              type="checkbox"
              checked={p.includeOriginals}
              onChange={(e) => p.onIncludeOriginals(e.target.checked)}
              disabled={p.busy}
            />
            <span>Include originals (contains location)</span>
          </label>
          {p.includeOriginals ? (
            <p className="share-warning" role="note">
              Originals keep their GPS location and camera details, so anyone with the link can see where the photos
              were taken. RAW and HEIC files can only be shared this way.
            </p>
          ) : (
            <p className="muted small share-hint">
              Visitors download a JPEG copy without location or camera details. RAW and HEIC photos are not offered.
            </p>
          )}
        </>
      )}
    </div>
  );
}

function errorText(e: unknown, fallback: string): string {
  return e instanceof ApiError || e instanceof Error ? e.message : fallback;
}

/** Create a link to an album or to the selected photos, then show it with a copy button. */
export function ShareDialog({
  target,
  onClose,
  onCreated,
}: {
  target: ShareTarget;
  onClose: () => void;
  onCreated?: (share: ShareLink) => void;
}) {
  const id = useId();
  const ref = useModal(onClose);
  const [title, setTitle] = useState("");
  const [preset, setPreset] = useState<ExpiryPreset>("never");
  const [password, setPassword] = useState("");
  const [allowDownload, setAllowDownload] = useState(true);
  const [includeOriginals, setIncludeOriginals] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<ShareLink | null>(null);
  const [copied, setCopied] = useState<"yes" | "no" | null>(null);

  const tooMany = target.kind === "photos" && target.ids.length > MAX_SHARE_PHOTOS;
  const what =
    target.kind === "album" ? `the album ${target.albumTitle}` : photoCount(target.kind === "photos" ? target.ids.length : 0);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (tooMany) return;
    setBusy(true);
    setError(null);
    try {
      const share = await shareApi.create({
        ...(target.kind === "album" ? { album_id: target.albumId } : { asset_ids: target.ids }),
        title: title.trim() || undefined,
        expires_at: expiryFromPreset(preset),
        password: password || undefined,
        allow_download: allowDownload,
        include_originals: allowDownload && includeOriginals,
      });
      setCreated(share);
      onCreated?.(share);
      setCopied((await copyText(shareUrl(share.token))) ? "yes" : "no");
    } catch (err) {
      setError(errorText(err, "Could not create the link."));
    } finally {
      setBusy(false);
    }
  }

  if (created) {
    const url = shareUrl(created.token);
    return (
      <div className="modal-backdrop">
        <form
          className="modal card share-dialog"
          role="dialog"
          aria-modal="true"
          aria-labelledby={`${id}-head`}
          ref={ref}
          onSubmit={(e) => {
            e.preventDefault();
            onClose();
          }}
        >
          <h2 id={`${id}-head`}>Link created</h2>
          <label htmlFor={`${id}-url`} className="sr-only">
            Share link
          </label>
          <input
            id={`${id}-url`}
            className="text-input share-url"
            readOnly
            value={url}
            onFocus={(e) => e.currentTarget.select()}
          />
          <p className="muted small share-hint" role="status">
            {copied === "yes" ? "Copied to the clipboard. " : copied === "no" ? "Copy the link above. " : ""}
            Anyone with it can view {created.kind === "album" ? "the album" : photoCount(created.item_count)}
            {created.has_password ? ", after entering the password" : ""}. {expiryText(created.expires_at)}.
          </p>
          <div className="modal-actions">
            <Link to="/shared" className="btn btn-quiet" onClick={onClose}>
              Shared links
            </Link>
            <button
              type="button"
              className="btn btn-quiet"
              onClick={() => void copyText(url).then((ok) => setCopied(ok ? "yes" : "no"))}
            >
              Copy link
            </button>
            <button type="submit" className="btn btn-primary">
              Done
            </button>
          </div>
        </form>
      </div>
    );
  }

  return (
    <div className="modal-backdrop">
      <form className="modal card share-dialog" role="dialog" aria-modal="true" aria-labelledby={`${id}-head`} ref={ref} onSubmit={(e) => void submit(e)}>
        <h2 id={`${id}-head`}>Share {what}</h2>
        {target.kind === "album" && (
          <p className="muted small share-hint">Photos you add to the album later appear on the link.</p>
        )}
        <Fields
          idPrefix={id}
          busy={busy}
          titlePlaceholder={target.kind === "album" ? target.albumTitle : "Shared photos"}
          title={title}
          onTitle={setTitle}
          expiry={
            <>
              <label htmlFor={`${id}-expiry`}>Link expires</label>
              <select
                id={`${id}-expiry`}
                className="text-input"
                value={preset}
                onChange={(e) => setPreset(e.target.value as ExpiryPreset)}
                disabled={busy}
              >
                {EXPIRY_PRESETS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                  </option>
                ))}
              </select>
            </>
          }
          password={password}
          onPassword={setPassword}
          passwordLabel="Password (optional)"
          passwordHint="At least 4 characters. Visitors must enter it to see anything."
          allowDownload={allowDownload}
          onAllowDownload={setAllowDownload}
          includeOriginals={includeOriginals}
          onIncludeOriginals={setIncludeOriginals}
        />
        {tooMany && (
          <p role="alert" className="form-error">
            A link holds up to {MAX_SHARE_PHOTOS} photos. Put them in an album and share the album instead.
          </p>
        )}
        {error && (
          <p role="alert" className="form-error">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy || tooMany}>
            Create link
          </button>
        </div>
      </form>
    </div>
  );
}

type EditExpiry = "keep" | ExpiryPreset;

/** Change a link's title, expiry, password and download settings. */
export function EditShareDialog({
  share,
  onClose,
  onSaved,
}: {
  share: ShareLink;
  onClose: () => void;
  onSaved: (share: ShareLink) => void;
}) {
  const id = useId();
  const ref = useModal(onClose);
  const [title, setTitle] = useState(share.title ?? "");
  const [expiry, setExpiry] = useState<EditExpiry>("keep");
  const [password, setPassword] = useState("");
  const [removePassword, setRemovePassword] = useState(false);
  const [allowDownload, setAllowDownload] = useState(share.allow_download);
  const [includeOriginals, setIncludeOriginals] = useState(share.include_originals);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const changes: Parameters<typeof shareApi.update>[1] = {
        title: title.trim() || null,
        allow_download: allowDownload,
        include_originals: allowDownload && includeOriginals,
      };
      if (expiry !== "keep") changes.expires_at = expiryFromPreset(expiry);
      if (removePassword) changes.password = null;
      else if (password) changes.password = password;
      onSaved(await shareApi.update(share.id, changes));
    } catch (err) {
      setError(errorText(err, "Could not save the changes."));
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop">
      <form className="modal card share-dialog" role="dialog" aria-modal="true" aria-labelledby={`${id}-head`} ref={ref} onSubmit={(e) => void submit(e)}>
        <h2 id={`${id}-head`}>Edit link</h2>
        <Fields
          idPrefix={id}
          busy={busy}
          titlePlaceholder={share.album?.title ?? "Shared photos"}
          title={title}
          onTitle={setTitle}
          expiry={
            <>
              <label htmlFor={`${id}-expiry`}>Link expires</label>
              <select
                id={`${id}-expiry`}
                className="text-input"
                value={expiry}
                onChange={(e) => setExpiry(e.target.value as EditExpiry)}
                disabled={busy}
              >
                <option value="keep">Keep: {expiryText(share.expires_at)}</option>
                {EXPIRY_PRESETS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.id === "never" ? "Never" : `${p.label} from now`}
                  </option>
                ))}
              </select>
            </>
          }
          password={password}
          onPassword={(v) => {
            setPassword(v);
            if (v) setRemovePassword(false);
          }}
          passwordLabel={share.has_password ? "New password" : "Password (optional)"}
          passwordHint={
            share.has_password
              ? "Leave empty to keep the current password. A new one signs everyone out of the link."
              : "At least 4 characters. Visitors must enter it to see anything."
          }
          extra={
            share.has_password && (
              <label className="share-check">
                <input
                  type="checkbox"
                  checked={removePassword}
                  onChange={(e) => {
                    setRemovePassword(e.target.checked);
                    if (e.target.checked) setPassword("");
                  }}
                  disabled={busy}
                />
                <span>Remove the password</span>
              </label>
            )
          }
          allowDownload={allowDownload}
          onAllowDownload={setAllowDownload}
          includeOriginals={includeOriginals}
          onIncludeOriginals={setIncludeOriginals}
        />
        {error && (
          <p role="alert" className="form-error">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy}>
            Save
          </button>
        </div>
      </form>
    </div>
  );
}
