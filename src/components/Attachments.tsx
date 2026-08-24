import type { Attachment } from "../lib/api";
import { toDataUri } from "../lib/images";

/**
 * Images attached to a message, before or after it is sent.
 *
 * The same strip in both places, because they are answering the same question —
 * *what am I sending / what did I send* — and a thumbnail that changed size and
 * shape between the composer and the transcript would read as a different
 * thing. Only the remove button differs, and it is absent once a message is
 * gone.
 *
 * Rendered from base64 rather than an object URL. The bytes are already in
 * memory on the way to the backend and already in the transcript on the way
 * back, so a URL would be a second handle on the same data with a lifetime to
 * manage — and one revoked too early is a broken image in a conversation that
 * has scrolled away.
 */
export function Attachments({
  images,
  onRemove,
}: {
  images: Attachment[];
  /** Omitted once the message is sent, which is what makes the strip static. */
  onRemove?: (index: number) => void;
}) {
  if (images.length === 0) return null;

  return (
    <div className="attachments">
      {images.map((image, i) => (
        <figure className="attachment" key={i}>
          <img
            src={toDataUri(image)}
            // The model was told nothing about this image beyond its bytes, so
            // there is no caption to borrow. The position is what a person
            // needs to match it against an error naming "image 2".
            alt={`Attached image ${i + 1}`}
          />
          {onRemove && (
            <button
              className="attachment-remove"
              aria-label={`Remove attached image ${i + 1}`}
              data-tip="Remove"
              onClick={() => onRemove(i)}
            >
              ✕
            </button>
          )}
        </figure>
      ))}
    </div>
  );
}
