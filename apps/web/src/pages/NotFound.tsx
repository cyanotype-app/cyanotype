import { Link } from "react-router-dom";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";

export function NotFound() {
  return (
    <>
      <PageHeader title="Page not found" />
      <div className="flex flex-col items-center gap-4 px-6 py-16 text-center">
        <p className="text-[0.9375rem] text-muted">There is nothing at this address.</p>
        <Button variant="primary" asChild>
          <Link to="/photos">Back to your photos</Link>
        </Button>
      </div>
    </>
  );
}
