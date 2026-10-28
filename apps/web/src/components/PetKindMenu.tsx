import { Ellipsis } from "lucide-react";
import { useCallback, useState } from "react";
import type { PhotoPet } from "../api";
import { Button } from "../ui/Button";
import { Menu, MenuItem } from "../ui/Menu";

/**
 * The detector can call a dog a cat. A small menu on an animal of the viewer's Pets row lets the owner say what it is: "It's a dog" or "It's a
 * cat" (the other kind than it is called now). Kept out of the way, behind a "⋯". On a phone the Info panel is a sheet (a modal dialog), so the
 * menu opens inside it.
 */
export function PetKindMenu({ animal, label, onKind }: { animal: PhotoPet; label: string; onKind: (animal: PhotoPet, kind: "dog" | "cat") => void }) {
  const other = animal.kind === "dog" ? "cat" : "dog";
  const [host, setHost] = useState<HTMLElement | null>(null);
  const find = useCallback((n: HTMLElement | null) => setHost(n?.closest("dialog") ?? null), []);
  return (
    <span ref={find} className="shrink-0">
      <Menu
        label={`${label} options`}
        container={host}
        trigger={
          <Button variant="ghost" size="icon-sm" aria-label={`More for ${label}`}>
            <Ellipsis />
          </Button>
        }
      >
        <MenuItem onSelect={() => onKind(animal, other)}>It’s a {other}</MenuItem>
      </Menu>
    </span>
  );
}
