import { Link } from "react-router-dom";

export function NotFound() {
  return (
    <div className="settings">
      <h1>Page not found</h1>
      <p className="muted">There is nothing at this address.</p>
      <p>
        <Link to="/photos">Back to your photos</Link>
      </p>
    </div>
  );
}
