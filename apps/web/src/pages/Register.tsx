import { useState } from "react";
import { Link, Navigate, useNavigate } from "react-router-dom";
import { api, ApiError } from "../api";
import { useAuth } from "../auth";
import { AuthForm } from "../components/AuthForm";
import { markRegistrationClosed, registrationKnownClosed } from "../registration";

export function Register() {
  const { state, refresh } = useAuth();
  const navigate = useNavigate();
  const [closed, setClosed] = useState(registrationKnownClosed());
  if (state.status === "authenticated") return <Navigate to="/library" replace />;

  return (
    <AuthForm
      title="Create account"
      submitLabel="Create account"
      passwordAutocomplete="new-password"
      hint="At least 8 characters."
      notice={
        closed && (
          <p className="notice" role="status">
            Registration is closed on this server. By default only the first account can be created; ask the owner to
            add you, or sign in if you already have an account.
          </p>
        )
      }
      onSubmit={async (email, password) => {
        try {
          await api.register(email, password);
        } catch (e) {
          if (e instanceof ApiError && e.status === 403) {
            markRegistrationClosed();
            setClosed(true);
            throw new Error("Registration is closed.", { cause: e });
          }
          if (e instanceof ApiError && e.status === 409) throw new Error("That email is already registered.", { cause: e });
          if (e instanceof ApiError && e.status === 429) throw new Error("Too many attempts. Wait a minute and try again.", { cause: e });
          throw e;
        }
        await refresh();
        navigate("/library", { replace: true });
      }}
      footer={
        <>
          Already have an account? <Link to="/login">Sign in</Link>
        </>
      }
    />
  );
}
