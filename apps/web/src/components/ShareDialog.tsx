import { Copy } from "lucide-react";
import { useId, useState, type FormEvent, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { ApiError, isScope, type Target, type ScopeTarget } from "../api";
import { photoCount } from "../albumText";
import { shareApi, type ShareLink } from "../shareApi";
import { EXPIRY_PRESETS, copyText, expiryFromPreset, expiryText, shareUrl, type ExpiryPreset } from "../shareText";
import { Button } from "../ui/Button";
import { Switch } from "../ui/Controls";
import { QrCode } from "./QrCode";
import { CheckRow, Field, FormError, Modal, Note, SelectBox, TextInput } from "./sheetForm";

/**
 * What a new link shares: a whole album (followed live), these photos (a fixed set), or, for "Select all", a view
 * minus some photos (`count` of them; the server resolves the set when the link is created).
 */
export type ShareTarget =
  | { kind: "album"; albumId: string; albumTitle: string }
  | { kind: "photos"; ids: readonly string[] }
  | { kind: "scope"; target: ScopeTarget; count: number };

/** The share target for a bulk-action target (`count` photos). */
export function shareTargetOf(target: Target, count: number): ShareTarget {
  return isScope(target) ? { kind: "scope", target, count } : { kind: "photos", ids: target };
}

const photosIn = (t: ShareTarget) => (t.kind === "photos" ? t.ids.length : t.kind === "scope" ? t.count : 0);

/** The most photos one link can hold (the server's bulk limit). */
export const MAX_SHARE_PHOTOS = 1000;

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
    <div className="flex flex-col gap-4">
      <Field label="Title (optional)" htmlFor={`${p.idPrefix}-title`}>
        <TextInput
          id={`${p.idPrefix}-title`}
          data-autofocus=""
          value={p.title}
          maxLength={200}
          placeholder={p.titlePlaceholder}
          onChange={(e) => p.onTitle(e.target.value)}
          disabled={p.busy}
        />
      </Field>
      {p.expiry}
      <Field label={p.passwordLabel} htmlFor={`${p.idPrefix}-password`} hint={p.passwordHint}>
        <TextInput
          id={`${p.idPrefix}-password`}
          type="password"
          autoComplete="new-password"
          value={p.password}
          maxLength={200}
          onChange={(e) => p.onPassword(e.target.value)}
          disabled={p.busy}
        />
      </Field>
      {p.extra}
      {/* (Radix adds a hidden checkbox beside a switch inside a form; nothing here reads it, and it would sit outside the sheet.) */}
      <div className="flex flex-col divide-y divide-line [&_input[aria-hidden=true]]:hidden">
        <Switch checked={p.allowDownload} onChange={p.onAllowDownload} disabled={p.busy} label="Allow downloads" />
        {p.allowDownload && (
          <Switch checked={p.includeOriginals} onChange={p.onIncludeOriginals} disabled={p.busy} label="Include originals (contains location)" />
        )}
      </div>
      {p.allowDownload &&
        (p.includeOriginals ? (
          <p className="rounded-card bg-surface p-3 text-sm text-danger" role="note">
            Originals keep their GPS location and camera details, so anyone with the link can see where the photos
            were taken. RAW and HEIC files can only be shared this way.
          </p>
        ) : (
          <Note>Visitors download a JPEG copy without location or camera details. RAW and HEIC photos are not offered.</Note>
        ))}
    </div>
  );
}

function errorText(e: unknown, fallback: string): string {
  return e instanceof ApiError || e instanceof Error ? e.message : fallback;
}

/** Create a link to an album or to the selected photos, then show it with a copy button and a QR code. */
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
  const [title, setTitle] = useState("");
  const [preset, setPreset] = useState<ExpiryPreset>("never");
  const [password, setPassword] = useState("");
  const [allowDownload, setAllowDownload] = useState(true);
  const [includeOriginals, setIncludeOriginals] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<ShareLink | null>(null);
  const [copied, setCopied] = useState<"yes" | "no" | null>(null);

  const tooMany = target.kind !== "album" && photosIn(target) > MAX_SHARE_PHOTOS;
  const what =
    target.kind === "album" ? `the album ${target.albumTitle}` : photoCount(photosIn(target));

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (tooMany) return;
    setBusy(true);
    setError(null);
    try {
      const share = await shareApi.create({
        ...(target.kind === "album"
          ? { album_id: target.albumId }
          : target.kind === "scope"
            ? { scope: target.target.scope, except: target.target.except }
            : { asset_ids: target.ids }),
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
      <Modal
        onClose={onClose}
        title="Link created"
        size="sm"
        footer={
          <>
            <Button asChild variant="quiet" className="md:mr-auto">
              <Link to="/shared" onClick={onClose}>
                Shared links
              </Link>
            </Button>
            <Button variant="secondary" onClick={() => void copyText(url).then((ok) => setCopied(ok ? "yes" : "no"))}>
              <Copy />
              Copy link
            </Button>
            <Button variant="primary" onClick={onClose} data-autofocus="">
              Done
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          <label htmlFor={`${id}-url`} className="sr-only">
            Share link
          </label>
          <TextInput id={`${id}-url`} readOnly value={url} onFocus={(e) => e.currentTarget.select()} />
          <div className="mx-auto w-44 rounded-card bg-white p-0 [&_svg]:m-0 [&_svg]:w-full [&_svg]:rounded-card [&_svg]:border-0">
            <QrCode text={url} label="QR code of the link" />
          </div>
          <Note role="status">
            {copied === "yes" ? "Copied to the clipboard. " : copied === "no" ? "Copy the link above. " : ""}
            Anyone with it can view {created.kind === "album" ? "the album" : photoCount(created.item_count)}
            {created.has_password ? ", after entering the password" : ""}. {expiryText(created.expires_at)}.
          </Note>
        </div>
      </Modal>
    );
  }

  return (
    <Modal
      onClose={onClose}
      title={`Share ${what}`}
      size="md"
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={id} disabled={busy || tooMany}>
            Create link
          </Button>
        </>
      }
    >
      <form id={id} className="flex flex-col gap-4" onSubmit={(e) => void submit(e)}>
        {target.kind === "album" && <Note>Photos you add to the album later appear on the link.</Note>}
        <Fields
          idPrefix={id}
          busy={busy}
          titlePlaceholder={target.kind === "album" ? target.albumTitle : "Shared photos"}
          title={title}
          onTitle={setTitle}
          expiry={
            <Field label="Link expires" htmlFor={`${id}-expiry`}>
              <SelectBox id={`${id}-expiry`} value={preset} onChange={(e) => setPreset(e.target.value as ExpiryPreset)} disabled={busy}>
                {EXPIRY_PRESETS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                  </option>
                ))}
              </SelectBox>
            </Field>
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
          <FormError>A link holds up to {MAX_SHARE_PHOTOS} photos. Put them in an album and share the album instead.</FormError>
        )}
        {error && <FormError>{error}</FormError>}
      </form>
    </Modal>
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
    <Modal
      onClose={onClose}
      title="Edit link"
      size="md"
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={id} disabled={busy}>
            Save
          </Button>
        </>
      }
    >
      <form id={id} className="flex flex-col gap-4" onSubmit={(e) => void submit(e)}>
        <Fields
          idPrefix={id}
          busy={busy}
          titlePlaceholder={share.album?.title ?? "Shared photos"}
          title={title}
          onTitle={setTitle}
          expiry={
            <Field label="Link expires" htmlFor={`${id}-expiry`}>
              <SelectBox id={`${id}-expiry`} value={expiry} onChange={(e) => setExpiry(e.target.value as EditExpiry)} disabled={busy}>
                <option value="keep">Keep: {expiryText(share.expires_at)}</option>
                {EXPIRY_PRESETS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.id === "never" ? "Never" : `${p.label} from now`}
                  </option>
                ))}
              </SelectBox>
            </Field>
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
              <CheckRow
                checked={removePassword}
                onChange={(on) => {
                  setRemovePassword(on);
                  if (on) setPassword("");
                }}
                disabled={busy}
              >
                Remove the password
              </CheckRow>
            )
          }
          allowDownload={allowDownload}
          onAllowDownload={setAllowDownload}
          includeOriginals={includeOriginals}
          onIncludeOriginals={setIncludeOriginals}
        />
        {error && <FormError>{error}</FormError>}
      </form>
    </Modal>
  );
}
