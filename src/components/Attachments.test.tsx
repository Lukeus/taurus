import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { Attachment } from "../lib/api";
import { Attachments } from "./Attachments";

const image = (data = "AAAA"): Attachment => ({ mime_type: "image/png", data });

describe("attached images", () => {
  it("draws each one from its own bytes", () => {
    // Rendered from base64 rather than an object URL: the bytes are already in
    // memory both on the way out and on the way back, and a URL revoked too
    // early is a broken image in a conversation that has scrolled away.
    const html = renderToStaticMarkup(
      <Attachments images={[image("AAAA"), image("BBBB")]} />,
    );
    expect(html).toContain("data:image/png;base64,AAAA");
    expect(html).toContain("data:image/png;base64,BBBB");
  });

  it("names each one by position", () => {
    // The model was told nothing about the image beyond its bytes, so there is
    // no caption to borrow — and the position is what matches a thumbnail
    // against an error naming "image 2".
    const html = renderToStaticMarkup(<Attachments images={[image(), image()]} />);
    expect(html).toContain("Attached image 1");
    expect(html).toContain("Attached image 2");
  });

  it("offers a way to take one back off while composing", () => {
    const html = renderToStaticMarkup(
      <Attachments images={[image()]} onRemove={() => {}} />,
    );
    expect(html).toContain("Remove attached image 1");
  });

  it("has no remove button once the message is sent", () => {
    // The strip is the same component in both places; the absence of the
    // callback is what makes the transcript copy static.
    const html = renderToStaticMarkup(<Attachments images={[image()]} />);
    expect(html).not.toContain("Remove attached image");
  });

  it("draws nothing at all when there is nothing attached", () => {
    // Not an empty strip — every message without an image renders this.
    expect(renderToStaticMarkup(<Attachments images={[]} />)).toBe("");
  });
});
