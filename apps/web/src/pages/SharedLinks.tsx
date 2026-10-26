import { Copy, ExternalLink, Link2, Pencil, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api";
import { useAuth } from "../auth";
import { ConfirmSheet } from "../components/ConfirmSheet";
import { Empty, InlineError, Loading, Rise } from "../components/pageKit";
import { inputClass } from "../components/sheetForm";
import { EditShareDialog } from "../components/ShareDialog";
import { useToast } from "../components/Toast";
import { shareApi, type ShareLink } from "../shareApi";
import { copyText, downloadText, shareName, shareSummary, shareUrl } from "../shareText";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";

/** The owner's share links: copy, edit expiry / password / downloads, revoke. */
export function SharedLinks() {
  const { setAnonymous } = useAuth();
  const { toast, show } = useToast();
  const [links, setLinks] = useState<ShareLink[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<ShareLink | null>(null);
  const [revoking, setRevoking] = useState<ShareLink | null>(null);
  const [busy, setBusy] = useState(false);

  const fail = useCallback(
    (e: unknown, fallback: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else return e instanceof Error ? e.message : fallback;
      return null;
    },
    [setAnonymous],
  );

  useEffect(() => {
    let live = true;
    shareApi
      .list()
      .then((r) => live && setLinks(r.items))
      .catch((e: unknown) => live && setError(fail(e, "Could not load your shared links.")));
    return () => {
      live = false;
    };
  }, [fail]);

  async function copy(s: ShareLink) {
    const ok = await copyText(shareUrl(s.token));
    show(ok ? { message: "Link copied" } : { message: "Could not copy. Open the link and copy it from the address bar.", tone: "error" });
  }

  async function revoke(s: ShareLink) {
    setBusy(true);
    try {
      await shareApi.revoke(s.id);
      setLinks((l) => l?.filter((x) => x.id !== s.id) ?? l);
      setRevoking(null);
      show({ message: "Link revoked. It no longer works." });
    } catch (e) {
      setRevoking(null);
      const msg = fail(e, "Could not revoke the link.");
      if (msg) show({ message: msg, tone: "error" });
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <PageHeader title="Shared links" />
      <div className="mx-auto max-w-[96rem] px-4 pt-3 pb-10 md:px-8">
        <p className="mb-4 max-w-prose text-sm text-muted">
          Anyone with a link can view what it shares, without an account. Revoking a link ends access at once.
        </p>
        {error && <InlineError>{error}</InlineError>}
        {!error && links === null && <Loading>Loading your links…</Loading>}
        {links !== null && links.length === 0 && (
          <Empty icon={Link2} title="No shared links yet">
            Select photos or open an album and choose Share to create one.
          </Empty>
        )}
        {links !== null && links.length > 0 && (
          <ul className="grid grid-cols-1 gap-3 lg:grid-cols-2" aria-label="Shared links">
              {links.map((s, i) => (
                <Rise key={s.id} index={i} className={cn("flex flex-col gap-3 rounded-card bg-card p-4 ring-1 ring-line", s.expired && "opacity-80")}>
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
                    <strong className="min-w-0 truncate text-[1.0625rem] font-semibold tracking-tight [overflow-wrap:anywhere]">{shareName(s)}</strong>
                    {s.expired && <Pill tone="warn">Expired</Pill>}
                    {s.has_password && <Pill>Password</Pill>}
                    {s.include_originals && <Pill tone="warn">Originals</Pill>}
                  </div>
                  <div className="flex flex-col gap-0.5 text-[0.8125rem] text-muted">
                    <p>{shareSummary(s)}</p>
                    <p>{downloadText(s)}</p>
                  </div>
                  <div className="flex items-center gap-2">
                    <input
                      className={cn(inputClass, "h-10 min-w-0 flex-1 rounded-full text-[0.8125rem] text-muted md:text-[0.8125rem]")}
                      readOnly
                      aria-label={`Link for ${shareName(s)}`}
                      value={shareUrl(s.token)}
                      onFocus={(e) => e.currentTarget.select()}
                    />
                    <Button variant="primary" onClick={() => void copy(s)}>
                      <Copy />
                      Copy link
                    </Button>
                  </div>
                  <div className="-mb-1 flex items-center gap-1 border-t border-line pt-2">
                    <Button asChild variant="quiet" size="sm">
                      <a href={shareUrl(s.token)} target="_blank" rel="noopener noreferrer" aria-label="Open">
                        <ExternalLink />
                        Open
                      </a>
                    </Button>
                    <Button variant="quiet" size="sm" onClick={() => setEditing(s)}>
                      <Pencil />
                      Edit
                    </Button>
                    <Button variant="quiet" size="sm" className="ml-auto text-danger hover:text-danger" onClick={() => setRevoking(s)}>
                      <Trash2 />
                      Revoke
                    </Button>
                  </div>
                </Rise>
              ))}
            </ul>
        )}
      </div>
      {editing && (
        <EditShareDialog
          share={editing}
          onClose={() => setEditing(null)}
          onSaved={(updated) => {
            setLinks((l) => l?.map((x) => (x.id === updated.id ? updated : x)) ?? l);
            setEditing(null);
            show({ message: "Link updated" });
          }}
        />
      )}
      {revoking && (
        <ConfirmSheet
          title="Revoke this link?"
          confirmLabel="Revoke link"
          danger
          busy={busy}
          onCancel={() => setRevoking(null)}
          onConfirm={() => void revoke(revoking)}
        >
          <p>Anyone who has the link will no longer be able to open it. Your photos are not touched.</p>
        </ConfirmSheet>
      )}
      {toast}
    </>
  );
}

function Pill({ tone, children }: { tone?: "warn"; children: string }) {
  return (
    <span className={cn("inline-flex h-6 items-center rounded-full bg-surface px-2.5 text-xs font-medium", tone === "warn" ? "text-danger" : "text-muted")}>{children}</span>
  );
}
