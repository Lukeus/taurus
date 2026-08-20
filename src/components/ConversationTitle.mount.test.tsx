// @vitest-environment jsdom
//
// Mounted rather than rendered to a string: every question here is about what
// a key does to a field that is only there once it has been clicked.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ConversationTitle } from "./ConversationTitle";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const onRename = vi.fn();

const mount = (title: string, renamable = true) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  act(() =>
    createRoot(host).render(
      <ConversationTitle
        title={title}
        renamable={renamable}
        onRename={onRename}
      />,
    ),
  );
  return host;
};

/** Opens the field, which is what a click on the title does. */
const open = (host: HTMLElement) => {
  const button = host.querySelector("button");
  if (!button) throw new Error(`no title button in: ${host.innerHTML}`);
  act(() => button.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  const input = host.querySelector("input");
  if (!input) throw new Error(`no field after clicking: ${host.innerHTML}`);
  return input;
};

const type = (input: HTMLInputElement, text: string) => {
  act(() => {
    // What React listens for; setting `.value` alone does not reach it.
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    setter.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
};

const press = (input: HTMLInputElement, key: string) =>
  act(() =>
    input.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true })),
  );

/**
 * Clicking away. `focusout` rather than `blur`, because that is the bubbling
 * event React's `onBlur` is actually built on — a `blur` dispatched here never
 * reaches the handler, and the test would pass by not testing anything.
 */
const blur = (input: HTMLInputElement) =>
  act(() => input.dispatchEvent(new FocusEvent("focusout", { bubbles: true })));

beforeEach(() => onRename.mockClear());

describe("the conversation title", () => {
  it("opens holding the name it is showing", () => {
    // Not empty. The common edit is a tweak to a title that is nearly right.
    expect(open(mount("fix the flaky test")).value).toBe("fix the flaky test");
  });

  it("saves on Enter", () => {
    const input = open(mount("fix the flaky test"));
    type(input, "Flaky CI investigation");
    press(input, "Enter");
    expect(onRename).toHaveBeenCalledWith("Flaky CI investigation");
  });

  it("saves on clicking away, which is the other way people finish", () => {
    const input = open(mount("old name"));
    type(input, "new name");
    blur(input);
    expect(onRename).toHaveBeenCalledWith("new name");
  });

  it("discards on Escape, and does not save it on the way out", () => {
    const input = open(mount("old name"));
    type(input, "never mind");
    press(input, "Escape");
    blur(input);
    expect(onRename).not.toHaveBeenCalled();
  });

  it("does nothing when the field is closed untouched", () => {
    // The most common way this ends, and it should cost a round trip to say
    // the conversation is called what it is already called.
    blur(open(mount("unchanged")));
    expect(onRename).not.toHaveBeenCalled();
  });

  it("treats surrounding whitespace as not a change", () => {
    const input = open(mount("a name"));
    type(input, "  a name  ");
    press(input, "Enter");
    expect(onRename).not.toHaveBeenCalled();
  });

  it("sends an empty title through, because that is how a name is cleared", () => {
    const input = open(mount("a question that was asked once"));
    type(input, "");
    press(input, "Enter");
    expect(onRename).toHaveBeenCalledWith("");
  });

  it("is plain text when there is no transcript to name yet", () => {
    const host = mount("New conversation", false);
    expect(host.querySelector("button")).toBeNull();
    expect(host.textContent).toBe("New conversation");
  });

  it("stays typable, rather than reselecting after every keystroke", () => {
    // A fresh callback ref each render would make React detach and reattach the
    // field, selecting all of it again between characters.
    const input = open(mount("a"));
    type(input, "ab");
    type(input, "abc");
    expect(input.value).toBe("abc");
    expect(input.selectionStart).toBe(input.selectionEnd);
  });
});
