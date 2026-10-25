// The face pieces of People: the cropped face, the "Who is this?" dialog, the people chips of the Info panel and that the viewer asks
// for a photo's faces only when People is on.
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type PhotoFace } from "../api";
import { detail, item, person, photoDetails } from "../test/fixtures";
import { PeopleProvider } from "../usePeople";
import { DetailsPanel } from "./DetailsPanel";
import { AssignFaceDialog } from "./FaceChips";
import { FaceCrop } from "./FaceCrop";
import { Lightbox } from "./Lightbox";

vi.mock("../map/PlaceMap", () => ({ default: () => <div data-testid="place-map" /> }));

afterEach(() => vi.restoreAllMocks());

describe("FaceCrop", () => {
  it("draws the preview big inside a square box once it knows the picture's size, and is decorative", () => {
    const { container } = render(<FaceCrop url="https://s3.test/p" box={[0.4, 0.4, 0.2, 0.2]} />);
    const box = container.querySelector(".face-crop")!;
    const img = box.querySelector("img")!;
    expect(img).toHaveAttribute("alt", "");
    expect(box).toHaveAttribute("data-ready", "false");
    expect(img.style.visibility).toBe("hidden");
    Object.defineProperty(img, "naturalWidth", { value: 1000 });
    Object.defineProperty(img, "naturalHeight", { value: 500 });
    fireEvent.load(img);
    expect(box).toHaveAttribute("data-ready", "true");
    // A 200 x 100 px face with 1.7 times its larger side around it: a 340 px square, the picture 1000 / 340 boxes wide.
    expect(Number.parseFloat(img.style.width)).toBeCloseTo((1000 / 340) * 100, 1);
    expect(box.className).toContain("round");
  });
  it("can be square", () => {
    const { container } = render(<FaceCrop url="u" box={[0, 0, 1, 1]} round={false} />);
    expect(container.querySelector(".face-crop")!.className).not.toContain("round");
  });
});

const face = (id: string, personRef: PhotoFace["person"]): PhotoFace => ({ id, box: [0.1, 0.1, 0.2, 0.2], person: personRef });

describe("AssignFaceDialog", () => {
  beforeEach(() => {
    vi.spyOn(api, "listPeople").mockResolvedValue({
      items: [person("p1", "Person A", 4), person("p2", null, 1), person("p3", "Person H", 2, { hidden: true })],
    });
  });
  it("makes a new person, with an optional name, by default", async () => {
    const assign = vi.spyOn(api, "assignFace").mockResolvedValue({ person: null });
    const done = vi.fn();
    render(<AssignFaceDialog face={face("f1", null)} onDone={done} onCancel={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Who is this?" });
    await userEvent.type(within(dialog).getByLabelText("Name (optional)"), " Person N ");
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Save" })).toBeEnabled());
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(done).toHaveBeenCalled());
    expect(assign).toHaveBeenCalledWith("f1", { new: true, name: "Person N" });
  });
  it("puts the face in a person from the list, and a blank name is left out", async () => {
    const assign = vi.spyOn(api, "assignFace").mockResolvedValue({ person: null });
    const done = vi.fn();
    render(<AssignFaceDialog face={face("f1", null)} onDone={done} onCancel={vi.fn()} />);
    const select = screen.getByLabelText("Person");
    // A new person, and the named, shown ones: never an unnamed or a hidden person.
    await waitFor(() => expect(within(select).getAllByRole("option")).toHaveLength(2));
    expect(within(select).queryByRole("option", { name: /Unnamed person|Person H/ })).not.toBeInTheDocument();
    await userEvent.selectOptions(select, "p1");
    expect(screen.queryByLabelText("Name (optional)")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(assign).toHaveBeenCalledWith("f1", { person_id: "p1" }));
    expect(done).toHaveBeenCalled();
  });
  it("shows a failure, keeps the dialog, and cancels with Escape", async () => {
    vi.spyOn(api, "assignFace").mockRejectedValue(new Error("offline"));
    const cancel = vi.fn();
    render(<AssignFaceDialog face={face("f1", null)} onDone={vi.fn()} onCancel={cancel} />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("offline");
    await userEvent.keyboard("{Escape}");
    expect(cancel).toHaveBeenCalled();
  });
});

describe("the people in the Info panel", () => {
  const panel = (faces: PhotoFace[] | undefined, onNameFace?: (f: PhotoFace) => void) =>
    render(
      <MemoryRouter>
        <DetailsPanel
          panelId="details-panel"
          state={{ status: "ready", details: photoDetails() }}
          onRetry={vi.fn()}
          onClose={vi.fn()}
          onLeave={() => Promise.resolve()}
          onNavigate={vi.fn()}
          faces={faces}
          onNameFace={onNameFace}
        />
      </MemoryRouter>,
    );
  beforeEach(() => {
    vi.spyOn(api, "tiles").mockResolvedValue({ world: false, detail: false });
  });
  it("shows a chip for each person, 'Unnamed person' for one without a name, each opening the person", () => {
    panel([face("f1", { id: "p1", name: "Person A" }), face("f2", { id: "p2", name: null })]);
    expect(screen.getByText("People")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Person A" })).toHaveAttribute("href", "/people/p1");
    expect(screen.getByRole("link", { name: "Unnamed person" })).toHaveAttribute("href", "/people/p2");
  });
  it("offers to name a face nobody has claimed", async () => {
    const name = vi.fn();
    panel([face("f1", null)], name);
    await userEvent.click(screen.getByRole("button", { name: "Name this face" }));
    expect(name).toHaveBeenCalledWith(face("f1", null));
  });
  it("has no People row without faces or while People is off", () => {
    panel(undefined);
    expect(screen.queryByText("People")).not.toBeInTheDocument();
    panel([]);
    expect(screen.queryByText("People")).not.toBeInTheDocument();
  });
});

describe("the viewer and People", () => {
  const items = [item(1)];
  function Harness({ on }: { on: boolean }) {
    const [i, setI] = useState(0);
    return (
      <MemoryRouter>
        <PeopleProvider>
          <Lightbox
            items={items}
            index={i}
            onIndexChange={setI}
            onClose={() => {}}
            loadDetail={async (id) => detail(Number(id.split("-")[1]))}
            loadDetails={async () => photoDetails()}
          />
          <p>{on ? "on" : "off"}</p>
        </PeopleProvider>
      </MemoryRouter>
    );
  }
  beforeEach(async () => {
    await new Promise((r) => setTimeout(r, 20));
    window.history.replaceState(null, "");
    vi.spyOn(api, "tiles").mockResolvedValue({ world: false, detail: false });
  });
  it("asks for the photo's faces when Info opens and People is on, and names one", async () => {
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "listPeople").mockResolvedValue({ items: [] });
    const faces = vi.spyOn(api, "photoFaces").mockResolvedValue({ items: [face("f1", { id: "p1", name: "Person A" }), face("f2", null)] });
    render(<Harness on />);
    expect(faces).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    expect(await screen.findByRole("link", { name: "Person A" })).toHaveAttribute("href", "/people/p1");
    expect(faces).toHaveBeenCalledWith("id-1");
    await userEvent.click(screen.getByRole("button", { name: "Name this face" }));
    expect(await screen.findByRole("dialog", { name: "Who is this?" })).toBeInTheDocument();
  });
  it("never asks while People is off", async () => {
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: false });
    const faces = vi.spyOn(api, "photoFaces");
    render(<Harness on={false} />);
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    await screen.findByText("Tuesday, July 4, 2023");
    await waitFor(() => expect(api.peopleStatus).toHaveBeenCalled());
    expect(faces).not.toHaveBeenCalled();
    expect(screen.queryByText("People")).not.toBeInTheDocument();
  });
});
