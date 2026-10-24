import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { api, ApiError, type Person } from "../api";
import { useAuth } from "../auth";
import { FaceCrop } from "../components/FaceCrop";
import { ADD_A_NAME, openPersonLabel, peopleWord, photosWord, splitPeople } from "../peopleText";
import { usePeopleEnabled } from "../usePeople";
import { NotFound } from "./NotFound";

/** One person: a round face, the name (or "Add a name") and how many photos. The whole card opens the person's page. */
export function PersonCard({ person }: { person: Person }) {
  return (
    <li>
      <Link to={`/people/${person.id}`} className="person-card" aria-label={openPersonLabel(person)}>
        {person.cover ? (
          <FaceCrop url={person.cover.preview_url} box={person.cover.box} className="person-card-face" />
        ) : (
          <span className="face-crop round person-card-face blank" aria-hidden="true" />
        )}
        <span className={`person-card-name${person.name ? "" : " muted"}`}>{person.name?.trim() || ADD_A_NAME}</span>
        <span className="person-card-count muted small">{photosWord(person.photos)}</span>
      </Link>
    </li>
  );
}

/** The people found in the owner's photos: a grid of round faces, the hidden ones in a collapsed section. */
export function PeoplePage() {
  const enabled = usePeopleEnabled();
  if (enabled === null) {
    return (
      <p className="center muted" role="status">
        Loading…
      </p>
    );
  }
  return enabled ? <PeopleList /> : <NotFound />;
}

function PeopleList() {
  const { setAnonymous } = useAuth();
  const [people, setPeople] = useState<Person[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .listPeople()
      .then((r) => live && setPeople(r.items))
      .catch((e: unknown) => {
        if (!live) return;
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        else setError(e instanceof Error ? e.message : "Could not load your people.");
      });
    return () => {
      live = false;
    };
  }, [setAnonymous]);

  const { shown, hidden } = useMemo(() => splitPeople(people ?? []), [people]);

  return (
    <div className="people-page">
      <div className="page-head">
        <div>
          <h1>People</h1>
          {people !== null && people.length > 0 && <p className="muted small">{peopleWord(shown.length)}</p>}
        </div>
      </div>
      {error && (
        <p className="center" role="alert">
          {error}
        </p>
      )}
      {!error && people === null && (
        <p className="center muted" role="status">
          Loading your people…
        </p>
      )}
      {people !== null && people.length === 0 && (
        <div className="empty">
          <h2>No people yet</h2>
          <p>
            Faces are found in the background, and a person appears once three photos show the same face. Only you can see this page; nothing here is
            shared.
          </p>
        </div>
      )}
      {shown.length > 0 && (
        <ul className="people-grid" aria-label="People">
          {shown.map((p) => (
            <PersonCard key={p.id} person={p} />
          ))}
        </ul>
      )}
      {hidden.length > 0 && (
        <details className="people-hidden">
          <summary>Hidden people ({hidden.length})</summary>
          <ul className="people-grid" aria-label="Hidden people">
            {hidden.map((p) => (
              <PersonCard key={p.id} person={p} />
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
