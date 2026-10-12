import { Link, Navigate, useLocation, useNavigate } from "react-router-dom";
import { api, ApiError } from "../api";
import { useAuth } from "../auth";
import { AuthForm } from "../components/AuthForm";
import { registrationKnownClosed } from "../registration";

export function Login() {
  const { state, refresh } = useAuth();
  const navigate = useNavigate();
  const from = (useLocation().state as { from?: string } | null)?.from ?? "/library";
  if (state.status === "authenticated") return <Navigate to="/library" replace />;

  return (
    <AuthForm
      title="Sign in"
      submitLabel="Sign in"
      passwordAutocomplete="current-password"
      onSubmit={async (email, password) => {
        try {
          await api.login(email, password);
        } catch (e) {
          if (e instanceof ApiError && e.status === 401) throw new Error("Wrong email or password.", { cause: e });
          if (e instanceof ApiError && e.status === 429) throw new Error("Too many attempts. Wait a minute and try again.", { cause: e });
          throw e;
        }
        await refresh();
        navigate(from, { replace: true });
      }}
      footer={
        registrationKnownClosed() ? (
          "Registration is closed on this server."
        ) : (
          <>
            New here? <Link to="/register">Create an account</Link>
          </>
        )
      }
    />
  );
}
