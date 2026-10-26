import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from "react";
import { ACCEPT, filterFiles } from "../upload/files";
import { useUploads } from "../upload/UploadProvider";

interface Picker {
  /** Open the system file picker (`folder`: a whole folder, where the browser offers it). */
  pick: (kind?: "files" | "folder") => void;
  /** Queue files from anywhere (a drop, say). Unsupported types are counted, not uploaded. */
  add: (files: File[]) => void;
  /** "3 files were skipped…" after the last pick, until dismissed. */
  notice: string | null;
  dismissNotice: () => void;
}

const PickerContext = createContext<Picker | null>(null);

/** One pair of hidden file inputs for the whole app, so any page's or the sidebar's Upload opens the same picker. */
export function UploadPickerProvider({ children }: { children: ReactNode }) {
  const { queue } = useUploads();
  const files = useRef<HTMLInputElement>(null);
  const folder = useRef<HTMLInputElement>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const add = useCallback(
    (list: File[]) => {
      const { accepted, skipped } = filterFiles(list);
      if (accepted.length) queue.add(accepted);
      setNotice(skipped ? `${skipped} file${skipped === 1 ? " was" : "s were"} skipped (not a supported photo type).` : null);
    },
    [queue],
  );
  const pick = useCallback((kind: "files" | "folder" = "files") => {
    (kind === "folder" ? folder : files).current?.click();
  }, []);
  const value = useMemo(() => ({ pick, add, notice, dismissNotice: () => setNotice(null) }), [pick, add, notice]);

  const onChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    add(Array.from(e.target.files ?? []));
    e.target.value = ""; // the same files may be chosen again
  };
  return (
    <PickerContext.Provider value={value}>
      {children}
      <input ref={files} type="file" multiple accept={ACCEPT} hidden onChange={onChange} aria-label="Choose photos" />
      <input ref={folder} type="file" multiple hidden onChange={onChange} aria-label="Choose a folder" {...{ webkitdirectory: "" }} />
    </PickerContext.Provider>
  );
}

export function useUploadPicker(): Picker {
  const v = useContext(PickerContext);
  if (!v) throw new Error("useUploadPicker must be used inside <UploadPickerProvider>");
  return v;
}
