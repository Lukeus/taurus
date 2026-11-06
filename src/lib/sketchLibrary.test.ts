import { beforeEach, describe, expect, it, vi } from "vitest";

const readSketchLibrary = vi.fn();
const saveSketchLibrary = vi.fn();
vi.mock("./api", () => ({
  readSketchLibrary: () => readSketchLibrary(),
  saveSketchLibrary: (text: string) => saveSketchLibrary(text),
}));

import { libraryAdapter, libraryFile, libraryItemsOf, LIBRARY_FILE } from "./sketchLibrary";

const ITEM = { id: "box", status: "unpublished", elements: [], created: 1 };

beforeEach(() => {
  readSketchLibrary.mockReset();
  saveSketchLibrary.mockReset().mockResolvedValue(undefined);
});

describe("the sketch library file", () => {
  it("reads back what it writes", () => {
    expect(libraryItemsOf(libraryFile([ITEM]))).toEqual([ITEM]);
    expect(JSON.parse(libraryFile([ITEM]))).toMatchObject({ type: "excalidrawlib", version: 2 });
  });

  it("reads Excalidraw's first version, whose items were under `library`", () => {
    const v1 = JSON.stringify({ type: "excalidrawlib", version: 1, library: [[{ id: "e" }]] });
    expect(libraryItemsOf(v1)).toEqual([[{ id: "e" }]]);
  });

  it("isn't anything else", () => {
    for (const text of ["", "[]", "{", '{"type":"excalidraw","elements":[]}', '{"type":"excalidrawlib"}']) {
      expect(libraryItemsOf(text), text).toBeNull();
    }
  });
});

describe("the library adapter", () => {
  it("loads nothing before anything was kept, then saves what's added", async () => {
    readSketchLibrary.mockResolvedValue(null);
    const report = vi.fn();
    const adapter = libraryAdapter<unknown[], unknown[]>(report);
    expect(await adapter.load()).toBeNull();
    await adapter.save({ libraryItems: [ITEM] });
    expect(libraryItemsOf(saveSketchLibrary.mock.calls[0][0])).toEqual([ITEM]);
    expect(report).not.toHaveBeenCalled();
  });

  it("loads what the file holds", async () => {
    readSketchLibrary.mockResolvedValue(libraryFile([ITEM]));
    const adapter = libraryAdapter<unknown[], unknown[]>(vi.fn());
    expect(await adapter.load()).toEqual({ libraryItems: [ITEM] });
  });

  it("never writes over a file it couldn't read, and says so once", async () => {
    readSketchLibrary.mockResolvedValue("{ half a library");
    const report = vi.fn();
    const adapter = libraryAdapter<unknown[], unknown[]>(report);
    expect(await adapter.load()).toBeNull();
    // Excalidraw reads again before it saves; that's still one message.
    await adapter.load();
    await adapter.save({ libraryItems: [ITEM] });
    expect(saveSketchLibrary).not.toHaveBeenCalled();
    expect(report).toHaveBeenCalledTimes(1);
    expect(report.mock.calls[0][0]).toContain(LIBRARY_FILE);
    expect(report.mock.calls[0][0]).toContain("Fix or delete the file");
  });

  it("treats a failed read the same way", async () => {
    readSketchLibrary.mockRejectedValue("Couldn't read /h/.taurus/sketch-library.excalidrawlib: denied");
    const report = vi.fn();
    const adapter = libraryAdapter<unknown[], unknown[]>(report);
    expect(await adapter.load()).toBeNull();
    await adapter.save({ libraryItems: [ITEM] });
    expect(saveSketchLibrary).not.toHaveBeenCalled();
    expect(report.mock.calls[0][0]).toMatch(/^Couldn't read .*won't be saved until it can be read\.$/);
  });

  it("says why a save was refused", async () => {
    readSketchLibrary.mockResolvedValue(null);
    saveSketchLibrary.mockRejectedValue("Couldn't save /h/.taurus/sketch-library.excalidrawlib: disk full");
    const report = vi.fn();
    const adapter = libraryAdapter<unknown[], unknown[]>(report);
    await adapter.load();
    await adapter.save({ libraryItems: [ITEM] });
    expect(report).toHaveBeenCalledWith("Couldn't save /h/.taurus/sketch-library.excalidrawlib: disk full");
  });
});
