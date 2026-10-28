// The face pieces of People: the cropped face, the "Who is this?" dialog, the people chips of the Info panel and that the viewer asks
// for a photo's faces only when People is on.
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ComponentProps } from "react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type PhotoFace } from "../api";
import { detail, item, person, photoDetails } from "../test/fixtures";
import { PeopleProvider } from "../usePeople";
import { DetailsPanel } from "./DetailsPanel";
import { AssignFaceDialog, ConfirmFaceDialog } from "./FaceChips";
import { FaceInPhoto } from "./FaceInPhoto";
import { FaceRegions } from "./FaceRegions";
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
  it("shows the whole photo with the face outlined when it knows the picture", () => {
    render(<AssignFaceDialog face={{ ...face("f1", null), box: [0.25, 0.1, 0.5, 0.4] }} previewUrl="https://s3.test/preview" onDone={vi.fn()} onCancel={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Who is this?" });
    expect(within(dialog).getByRole("img", { name: /the photo, with the face in question outlined/i })).toHaveAttribute("src", "https://s3.test/preview");
    const box = within(dialog).getByTestId("face-context-box");
    expect(box.style.left).toBe("25%");
    expect(box.style.top).toBe("10%");
    expect(box.style.width).toBe("50%");
    expect(box.style.height).toBe("40%");
  });
  it("has no picture without a preview, and no Not a face without somewhere to send it", () => {
    render(<AssignFaceDialog face={face("f1", null)} onDone={vi.fn()} onCancel={vi.fn()} />);
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Not a face" })).not.toBeInTheDocument();
  });
  it("says it is not a face, and tells the caller who it was in", async () => {
    const mark = vi.spyOn(api, "markNotAFace").mockResolvedValue({ person_id: null });
    const marked = vi.fn();
    const assign = vi.spyOn(api, "assignFace");
    render(<AssignFaceDialog face={face("f1", null)} onDone={vi.fn()} onNotAFace={marked} onCancel={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "Not a face" }));
    await waitFor(() => expect(marked).toHaveBeenCalledWith(face("f1", null), null));
    expect(mark).toHaveBeenCalledWith("f1");
    expect(assign).not.toHaveBeenCalled();
  });
  it("keeps the dialog and shows why when Not a face fails", async () => {
    vi.spyOn(api, "markNotAFace").mockRejectedValue(new Error("offline"));
    const marked = vi.fn();
    render(<AssignFaceDialog face={face("f1", null)} onDone={vi.fn()} onNotAFace={marked} onCancel={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "Not a face" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("offline");
    expect(marked).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Not a face" })).toBeEnabled();
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

describe("FaceInPhoto", () => {
  it("puts the box on the face as fractions of the picture, and the picture says what it shows", () => {
    render(<FaceInPhoto url="https://s3.test/p" box={[0.123456, 0.5, 0.2, 0.25]} label="Alpha in the group" />);
    expect(screen.getByRole("img", { name: "Alpha in the group" })).toHaveAttribute("src", "https://s3.test/p");
    const box = screen.getByTestId("face-context-box");
    expect(box).toHaveAttribute("aria-hidden", "true");
    expect([box.style.left, box.style.top, box.style.width, box.style.height]).toEqual(["12.35%", "50%", "20%", "25%"]);
  });
});

describe("ConfirmFaceDialog", () => {
  const open = (extra: Partial<Parameters<typeof ConfirmFaceDialog>[0]> = {}) => {
    const fns = { onYes: vi.fn().mockResolvedValue(undefined), onNo: vi.fn().mockResolvedValue(undefined), onNotAFace: vi.fn().mockResolvedValue(undefined), onCancel: vi.fn() };
    render(<ConfirmFaceDialog person={{ name: "Person A" }} previewUrl="https://s3.test/p" box={[0.1, 0.2, 0.3, 0.4]} {...fns} {...extra} />);
    return fns;
  };
  it("asks about the person by name over the whole photo with the face outlined", () => {
    open();
    const dialog = screen.getByRole("dialog", { name: "Is this Person A?" });
    expect(within(dialog).getByRole("img")).toHaveAttribute("src", "https://s3.test/p");
    expect(within(dialog).getByTestId("face-context-box").style.left).toBe("10%");
  });
  it("asks about the same person when there is no name", () => {
    open({ person: { name: null } });
    expect(screen.getByRole("dialog", { name: "Is this the same person?" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Yes, this is Unnamed person" })).toBeInTheDocument();
  });
  it("has Yes, No and Not a face, each its own answer", async () => {
    for (const [button, handler] of [
      ["Yes, this is Person A", "onYes"],
      ["No, this is not Person A", "onNo"],
      ["Not a face", "onNotAFace"],
    ] as const) {
      const f = open();
      await userEvent.click(screen.getByRole("button", { name: button }));
      for (const h of ["onYes", "onNo", "onNotAFace"] as const) expect(f[h]).toHaveBeenCalledTimes(h === handler ? 1 : 0);
      cleanup();
    }
  });
  it("shows why an answer failed and lets the owner try again, and Escape cancels", async () => {
    const f = open({ onYes: vi.fn().mockRejectedValue(new Error("offline")) });
    await userEvent.click(screen.getByRole("button", { name: "Yes, this is Person A" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("offline");
    expect(screen.getByRole("button", { name: "Yes, this is Person A" })).toBeEnabled();
    await userEvent.keyboard("{Escape}");
    expect(f.onCancel).toHaveBeenCalled();
  });
});

describe("FaceRegions", () => {
  const rect = { left: 10, top: 20, width: 400, height: 300 };
  const named = face("f1", { id: "p1", name: "Person A" });
  const unclaimed = { ...face("f2", null), box: [0.5, 0.25, 0.1, 0.2] as PhotoFace["box"] };
  const unnamed = face("f3", { id: "p2", name: null });
  const draw = (over: { showAll?: boolean; onOpen?: (f: PhotoFace) => void; onNotAFace?: (f: PhotoFace) => void } = {}) => {
    const onOpen = over.onOpen ?? vi.fn();
    const onNotAFace = over.onNotAFace ?? vi.fn();
    const { container } = render(<FaceRegions faces={[named, unclaimed, unnamed]} rect={rect} showAll={over.showAll ?? false} onOpen={onOpen} onNotAFace={onNotAFace} />);
    return { container, onOpen, onNotAFace };
  };
  it("lays the layer over the picture and each face over its box", () => {
    const { container } = draw();
    const layer = container.querySelector<HTMLElement>(".face-regions")!;
    expect([layer.style.left, layer.style.top, layer.style.width, layer.style.height]).toEqual(["10px", "20px", "400px", "300px"]);
    const spot = container.querySelector<HTMLElement>('[data-face-id="f2"]')!.closest<HTMLElement>(".face-spot")!;
    expect([spot.style.left, spot.style.top, spot.style.width, spot.style.height]).toEqual(["50%", "25%", "10%", "20%"]);
  });
  it("is a link for a named person and a button for any other face, each labelled, each reachable with the keyboard", () => {
    draw();
    const link = screen.getByRole("link", { name: "Person A: open their page" });
    expect(link).toHaveAttribute("tabindex", "0");
    expect(link).toHaveAttribute("data-face-id", "f1");
    expect(link).toHaveAttribute("data-tap");
    expect(screen.getAllByRole("button", { name: "Unknown face: who is this?" })).toHaveLength(2);
  });
  it("opens with Enter and Space, and with the click an assistive technology makes, but not on a pointer's own click (the viewer hears those)", async () => {
    const { onOpen } = draw();
    const region = screen.getByRole("link", { name: "Person A: open their page" });
    region.focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.keyboard(" ");
    expect(onOpen).toHaveBeenCalledTimes(2);
    expect(onOpen).toHaveBeenLastCalledWith(named);
    fireEvent.click(region);
    expect(onOpen).toHaveBeenCalledTimes(3);
    fireEvent.click(region, { detail: 1 });
    expect(onOpen).toHaveBeenCalledTimes(3);
  });
  it("keeps the keys it handles to itself, so the viewer does not also page or zoom", () => {
    draw();
    const heard = vi.fn();
    document.addEventListener("keydown", heard);
    fireEvent.keyDown(screen.getByRole("link", { name: "Person A: open their page" }), { key: "Enter" });
    document.removeEventListener("keydown", heard);
    expect(heard).not.toHaveBeenCalled();
  });
  it("outlines every face only when asked to", () => {
    const { container } = draw();
    expect(container.querySelector(".face-regions")).not.toHaveClass("all");
    cleanup();
    const again = draw({ showAll: true });
    expect(again.container.querySelector(".face-regions")).toHaveClass("all");
  });
  it("offers Not a face for each face", async () => {
    const { onNotAFace } = draw();
    const buttons = screen.getAllByRole("button", { name: "Not a face" });
    expect(buttons).toHaveLength(3);
    await userEvent.click(buttons[0]!);
    expect(onNotAFace).toHaveBeenCalledWith(named);
  });
});

describe("the people in the Info panel", () => {
  const panel = (faces: PhotoFace[] | undefined, extra: Partial<ComponentProps<typeof DetailsPanel>> = {}) =>
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
          {...extra}
        />
      </MemoryRouter>,
    );
  beforeEach(() => {
    vi.spyOn(api, "tiles").mockResolvedValue({ world: false, detail: false });
  });
  it("shows a chip for each named person, once, each opening the person", () => {
    panel([face("f1", { id: "p1", name: "Person A" }), face("f2", { id: "p1", name: "Person A" }), face("f3", { id: "p3", name: "Person B" })]);
    expect(screen.getByText("People")).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: "Person A" })).toHaveLength(1);
    expect(screen.getByRole("link", { name: "Person A" })).toHaveAttribute("href", "/people/p1");
    expect(screen.getByRole("link", { name: "Person B" })).toHaveAttribute("href", "/people/p3");
  });
  it("does not list unnamed people or faces nobody has claimed", () => {
    panel([face("f1", { id: "p2", name: null }), face("f2", { id: "p4", name: "  " }), face("f3", null)]);
    expect(screen.queryByText("People")).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Unnamed person" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Name this face" })).not.toBeInTheDocument();
    expect(screen.queryByText("Unknown face")).not.toBeInTheDocument();
  });
  it("has no People row without faces or while People is off", () => {
    panel(undefined);
    expect(screen.queryByText("People")).not.toBeInTheDocument();
    cleanup();
    panel([]);
    expect(screen.queryByText("People")).not.toBeInTheDocument();
  });
  it("switches the face boxes on the photo on and off when the photo has faces to click", async () => {
    const set = vi.fn();
    panel([face("f1", null)], { regionBoxes: false, onRegionBoxes: set });
    const box = screen.getByRole("switch", { name: "Show faces on the photo" });
    expect(box).not.toBeChecked();
    await userEvent.click(box);
    expect(set).toHaveBeenCalledWith(true);
  });
  it("words the one switch by what the photo has: faces, pets, or both", () => {
    const animals = { animals: [{ id: "d1", kind: "dog" as const, box: [0.1, 0.1, 0.2, 0.2] as [number, number, number, number], score: 0.9, pet: null }], picture: null, onAdd: vi.fn(), onNot: vi.fn() };
    panel([face("f1", null)], { onRegionBoxes: vi.fn(), pets: animals });
    expect(screen.getAllByRole("switch")).toHaveLength(1);
    expect(screen.getByRole("switch", { name: "Show faces and pets on the photo" })).toBeInTheDocument();
    cleanup();
    panel([], { onRegionBoxes: vi.fn(), pets: animals });
    expect(screen.getByRole("switch", { name: "Show pets on the photo" })).toBeInTheDocument();
    cleanup();
    panel(undefined, { onRegionBoxes: vi.fn() });
    expect(screen.queryByRole("switch")).not.toBeInTheDocument();
  });
  it("offers no switch for the boxes when there is nothing to outline", () => {
    panel([], { onRegionBoxes: vi.fn() });
    expect(screen.queryByRole("switch", { name: "Show faces on the photo" })).not.toBeInTheDocument();
  });
  it("brings back detections marked Not a face", async () => {
    const back = vi.fn();
    panel([], { dismissed: 2, onBringBack: back });
    await userEvent.click(screen.getByRole("button", { name: /Bring back 2 detections marked/ }));
    expect(back).toHaveBeenCalled();
  });
  it("says 1 detection for one", () => {
    panel([], { dismissed: 1, onBringBack: vi.fn() });
    expect(screen.getByRole("button", { name: /Bring back 1 detection marked/ })).toBeInTheDocument();
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
  it("asks for the photo's faces as soon as it opens when People is on, and lists the named people in Info", async () => {
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "listPeople").mockResolvedValue({ items: [] });
    const faces = vi.spyOn(api, "photoFaces").mockResolvedValue({ items: [face("f1", { id: "p1", name: "Person A" }), face("f2", null)], dismissed: [] });
    render(<Harness on />);
    await waitFor(() => expect(faces).toHaveBeenCalledWith("id-1"));
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    expect(await screen.findByRole("link", { name: "Person A" })).toHaveAttribute("href", "/people/p1");
    expect(screen.queryByRole("button", { name: "Name this face" })).not.toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Show faces on the photo" })).not.toBeChecked();
  });
  it("brings back what was marked Not a face from the Info panel, and reads the faces again", async () => {
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: true });
    const faces = vi
      .spyOn(api, "photoFaces")
      .mockResolvedValueOnce({ items: [], dismissed: [{ id: "d1", box: [0.1, 0.1, 0.2, 0.2] }] })
      .mockResolvedValue({ items: [face("d1", null)], dismissed: [] });
    const unmark = vi.spyOn(api, "unmarkNotAFace").mockResolvedValue(undefined);
    render(<Harness on />);
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    await userEvent.click(await screen.findByRole("button", { name: /Bring back 1 detection marked/ }));
    expect(unmark).toHaveBeenCalledWith("d1");
    await waitFor(() => expect(faces).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("button", { name: /Bring back/ })).not.toBeInTheDocument());
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
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
