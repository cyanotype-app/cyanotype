import { useCallback, useEffect, useState } from "react";
import { api, type SessionInfo } from "../api";
import { useAuthOptional } from "../auth";
import { FormError, Group, Loading, Row, Section, StatusNote } from "../settings/kit";
import { Button } from "../ui/Button";

const when = (iso: string) => new Date(iso).toLocaleString();

/** Settings > Sessions: where the account is signed in, with "sign out everywhere". */
export function Sessions() {
  const auth = useAuthOptional();
  const [items, setItems] = useState<SessionInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setItems((await api.mySessions()).items);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load your sessions.");
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  async function run(what: () => Promise<unknown>, done: string, signedOut = false) {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      await what();
      if (signedOut) {
        auth?.setAnonymous();
        return;
      }
      setNote(done);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "That did not work.");
    } finally {
      setBusy(false);
    }
  }

  const others = items?.filter((s) => !s.current).length ?? 0;
  return (
    <div>
      <p className="text-sm text-muted">Each browser you signed in on is a session; it lasts 30 days. End the ones you do not recognise.</p>
      <div className="mt-3 flex flex-col gap-2">
        <FormError>{error}</FormError>
        <StatusNote>{note}</StatusNote>
      </div>
      {items === null ? (
        !error && <Loading />
      ) : (
        <>
          <Section className="mt-4">
            <Group as="ul">
              {items.map((s) => (
                <Row
                  as="li"
                  key={s.id}
                  label={<strong className="font-medium">{s.current ? "This browser" : "Another browser"}</strong>}
                  detail={
                    <>
                      Signed in {when(s.created_at)} · expires {when(s.expires_at)}
                    </>
                  }
                >
                  {!s.current && (
                    <Button size="sm" disabled={busy} onClick={() => void run(() => api.revokeSession(s.id), "Signed out.")}>
                      Sign out
                    </Button>
                  )}
                </Row>
              ))}
            </Group>
          </Section>
          <div className="mt-5 flex flex-wrap gap-2">
            {others > 0 && (
              <Button disabled={busy} onClick={() => void run(() => api.revokeOtherSessions(), "Signed out everywhere else.")}>
                Sign out the others
              </Button>
            )}
            <Button variant="quiet" disabled={busy} onClick={() => void run(() => api.revokeAllSessions(), "", true)}>
              Sign out everywhere
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
