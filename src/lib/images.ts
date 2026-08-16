/**
 * Turning what a browser hands you into what the backend takes.
 *
 * A paste and a drop both produce `File` objects, and a `File` is a stream of
 * bytes with a guessed type on it. The backend wants base64 and checks the
 * guess against the bytes, so everything here is transport: read, encode, and
 * refuse the obvious cases early enough that the user finds out while the file
 * is still in their hand.
 *
 * The real validation lives in `taurus_host::attach`, where both frontends and
 * every provider can share it. What is duplicated here is only what has to
 * happen before a round trip to be worth anything.
 */
import type { Attachment } from "./api";

/**
 * Matches `MAX_IMAGE_BYTES` in `taurus_host::attach`.
 *
 * Duplicated rather than fetched: the check exists to fail *before* a 40 MB
 * file is read into memory and base64-encoded, and a value that drifts costs a
 * clear message replaced by the backend's equally clear one.
 */
export const MAX_IMAGE_BYTES = 5 * 1024 * 1024;

/** Matches `MAX_IMAGES` in `taurus_host::attach`. */
export const MAX_IMAGES = 4;

/** The formats every backend accepts. Matches `ACCEPTED` in the same module. */
const ACCEPTED = ["image/png", "image/jpeg", "image/webp", "image/gif"];

export function isImage(file: File | null | undefined): boolean {
  return !!file && ACCEPTED.includes(file.type.toLowerCase());
}

/**
 * Reads one file as an attachment, or throws with something worth reading.
 *
 * The size check comes before the read: encoding a 40 MB file to tell the user
 * it is too big spends a second and a hundred megabytes of string to learn
 * something `File.size` already knew.
 */
export async function toAttachment(file: File): Promise<Attachment> {
  const type = file.type.toLowerCase();
  if (!ACCEPTED.includes(type)) {
    throw new Error(
      `${file.name || "That file"} is ${file.type || "of no known type"}. Use PNG, JPEG, WebP, or GIF.`,
    );
  }
  if (file.size > MAX_IMAGE_BYTES) {
    throw new Error(
      `${file.name || "That image"} is ${(file.size / (1024 * 1024)).toFixed(1)} MB, past the ${
        MAX_IMAGE_BYTES / (1024 * 1024)
      } MB limit. Scale it down or crop to the part that matters.`,
    );
  }

  return { mime_type: type, data: await encode(file) };
}

/**
 * Base64 without the `data:` prefix.
 *
 * `FileReader` produces a data URI and there is no API that produces the
 * payload alone, so the prefix is cut rather than avoided. Chunked through
 * `String.fromCharCode` would blow the argument limit on a multi-megabyte
 * image, which is the size this is for.
 */
async function encode(file: File): Promise<string> {
  const uri = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(new Error(`Could not read ${file.name}.`));
    reader.readAsDataURL(file);
  });

  const comma = uri.indexOf(",");
  if (comma < 0) throw new Error(`Could not read ${file.name}.`);
  return uri.slice(comma + 1);
}

/** A data URI, for putting an attachment back on screen. */
export function toDataUri(attachment: Attachment): string {
  return `data:${attachment.mime_type};base64,${attachment.data}`;
}

/**
 * Reads several files, keeping the ones that worked and the reasons the rest
 * did not.
 *
 * Partial success on purpose: dropping four screenshots where one is a HEIC
 * should attach three and say what happened to the fourth, rather than refuse
 * the lot and leave the user to work out which one was the problem.
 */
export async function toAttachments(
  files: File[],
  already: number,
): Promise<{ attachments: Attachment[]; errors: string[] }> {
  const attachments: Attachment[] = [];
  const errors: string[] = [];

  for (const file of files) {
    if (already + attachments.length >= MAX_IMAGES) {
      errors.push(
        `Only ${MAX_IMAGES} images fit in one message; ${file.name || "the rest"} was left out.`,
      );
      continue;
    }
    try {
      attachments.push(await toAttachment(file));
    } catch (e) {
      errors.push(e instanceof Error ? e.message : String(e));
    }
  }

  return { attachments, errors };
}
