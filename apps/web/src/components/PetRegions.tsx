import type { KeyboardEvent } from "react";
import type { PhotoPet } from "../api";
import { pct, type PictureRect } from "./FaceRegions";
import "../viewer.css";

/** What a pet's region says: the pet's name when the animal is in a named pet, else "Dog" or "Cat". */
export const petRegionName = (a: PhotoPet) => a.pet?.name.trim() || (a.kind === "dog" ? "Dog" : "Cat");

/** What a screen reader says for an animal on the photo. */
export const petRegionLabel = (a: PhotoPet) => (a.pet ? `${petRegionName(a)}: open their page` : `${petRegionName(a)}: add to a pet`);

/**
 * The animals of the photo on show as regions over the picture, laid out like the faces' (same classes, so the same zoom, outline and
 * focus rules): a link for an animal in a named pet, a button for one nobody named. Each carries a label with its name. A tap is heard
 * by the viewer's zoom handler (`data-tap`, `data-pet-id`); Enter, Space and an assistive technology's click activate here.
 */
export function PetRegions({ animals, rect, showAll, onOpen }: { animals: PhotoPet[]; rect: PictureRect; showAll: boolean; onOpen: (animal: PhotoPet) => void }) {
  const onKey = (a: PhotoPet) => (e: KeyboardEvent) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    e.stopPropagation();
    onOpen(a);
  };
  return (
    <div className={`face-regions pet-regions${showAll ? " all" : ""}`} style={{ left: rect.left, top: rect.top, width: rect.width, height: rect.height }}>
      {animals.map((a) => {
        const [x, y, w, h] = a.box;
        return (
          <span key={a.id} className="face-spot" style={{ left: pct(x), top: pct(y), width: pct(w), height: pct(h) }}>
            <span
              className="face-region"
              role={a.pet ? "link" : "button"}
              tabIndex={0}
              aria-label={petRegionLabel(a)}
              data-tap=""
              data-pet-id={a.id}
              onKeyDown={onKey(a)}
              onClick={(e) => {
                if (e.detail === 0) onOpen(a);
              }}
            />
            <span className="region-label" aria-hidden="true" style={{ padding: "0 0.5rem" }}>
              {petRegionName(a)}
            </span>
          </span>
        );
      })}
    </div>
  );
}
