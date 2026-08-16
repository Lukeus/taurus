// @vitest-environment jsdom
//
// `FileReader` and `File` are browser APIs, so these need a document even
// though nothing is rendered.
import { describe, expect, it } from "vitest";

import {
  MAX_IMAGES,
  MAX_IMAGE_BYTES,
  isImage,
  toAttachment,
  toAttachments,
  toDataUri,
} from "./images";

const png = (bytes: number[] = [0x89, 0x50, 0x4e, 0x47], name = "shot.png") =>
  new File([new Uint8Array(bytes)], name, { type: "image/png" });

describe("recognising an image", () => {
  it("takes the four formats every backend accepts", () => {
    for (const type of ["image/png", "image/jpeg", "image/webp", "image/gif"]) {
      expect(isImage(new File([], "x", { type }))).toBe(true);
    }
  });

  it("refuses one that only some backends take", () => {
    // HEIC works on Gemini and not on Anthropic. A format that works until the
    // day someone switches provider is worse than one that never worked.
    expect(isImage(new File([], "x", { type: "image/heic" }))).toBe(false);
  });

  it("refuses a file that is not an image at all", () => {
    expect(isImage(new File([], "notes.txt", { type: "text/plain" }))).toBe(false);
    expect(isImage(null)).toBe(false);
  });

  it("matches however the type was capitalized", () => {
    // Clipboard flavours arrive shouting on some platforms.
    expect(isImage(new File([], "x", { type: "IMAGE/PNG" }))).toBe(true);
  });
});

describe("encoding one file", () => {
  it("strips the data-URI prefix the reader adds", () => {
    // `FileReader` has no API that produces the payload alone, and a prefix
    // left on would reach the provider as part of the base64.
    return toAttachment(png()).then((attachment) => {
      expect(attachment.mime_type).toBe("image/png");
      expect(attachment.data.startsWith("data:")).toBe(false);
      expect(atob(attachment.data)).toBe("\x89PNG");
    });
  });

  it("round-trips back to a data URI for display", () => {
    return toAttachment(png()).then((attachment) => {
      expect(toDataUri(attachment)).toBe(`data:image/png;base64,${attachment.data}`);
    });
  });

  it("checks the size before reading the file, not after", async () => {
    // Encoding a 40 MB file to discover it is too big spends a second and a
    // hundred megabytes of string to learn what `File.size` already knew.
    const huge = new File([new Uint8Array(MAX_IMAGE_BYTES + 1)], "big.png", {
      type: "image/png",
    });
    await expect(toAttachment(huge)).rejects.toThrow(/past the 5 MB limit/);
  });

  it("names the file and the type it actually had", async () => {
    const wrong = new File([new Uint8Array([1])], "photo.heic", {
      type: "image/heic",
    });
    await expect(toAttachment(wrong)).rejects.toThrow(/photo\.heic is image\/heic/);
  });
});

describe("encoding a batch", () => {
  it("keeps the ones that worked and reports the rest", async () => {
    // Dropping four screenshots where one is a HEIC should attach three and
    // say what happened to the fourth, not refuse the lot.
    const { attachments, errors } = await toAttachments(
      [
        png(),
        new File([new Uint8Array([1])], "bad.heic", { type: "image/heic" }),
        png([0x89, 0x50], "second.png"),
      ],
      0,
    );
    expect(attachments).toHaveLength(2);
    expect(errors).toHaveLength(1);
    expect(errors[0]).toContain("bad.heic");
  });

  it("counts what is already attached against the limit", async () => {
    // The cap is per message, not per drop, so a second drop has to know about
    // the first.
    const { attachments, errors } = await toAttachments([png(), png()], MAX_IMAGES - 1);
    expect(attachments).toHaveLength(1);
    expect(errors[0]).toContain(`Only ${MAX_IMAGES} images`);
  });

  it("attaches nothing and says nothing for an empty batch", async () => {
    const { attachments, errors } = await toAttachments([], 0);
    expect(attachments).toEqual([]);
    expect(errors).toEqual([]);
  });
});
