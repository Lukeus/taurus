import { type ReactNode } from "react";

import { Modal } from "./Modal";

/**
 * The panel every list, report and editor in this app opens into.
 *
 * Ten components were writing this out: the same `Modal`, the same `<aside
 * className="drawer">`, the same header with the title on the left and the
 * dismiss on the right. Written ten times it drifted ten ways — three of them
 * spelled the close control as the word "Close" and seven as a ✕, which is one
 * control in one corner reading two different ways depending on which panel you
 * had open.
 *
 * The part that mattered most is invisible: `onClick={(e) =>
 * e.stopPropagation()}` on the panel. `Modal` dismisses on a click that reaches
 * the scrim, and without that line every click on a patch of drawer that is not
 * itself a control closes the drawer. It is one line, it is easy to leave out,
 * and leaving it out is not something a type checks or a test notices — it is
 * the drawer shutting under somebody's cursor. Written once, it cannot be left
 * out.
 *
 * Not every panel fits, and the ones that do not are left alone rather than
 * given a flag each: the changes pane is docked rather than floating and has no
 * scrim to stop a click reaching, the delegate transcript lives in a resizable
 * dock, and the MCP drawer's `<aside>` wraps three alternatives of which only
 * one has a header. All four still share the header below, which is the half
 * they actually have in common.
 */
export function Drawer({
  title,
  onClose,
  panel,
  actions,
  overlay,
  children,
}: {
  title: string;
  onClose: () => void;
  /** An extra class on the panel, for the three that size or colour their own. */
  panel?: string;
  /**
   * Controls between the title and the dismiss — a Rescan, a Reconnect.
   *
   * A slot rather than a prop per button, because what belongs in a drawer's
   * header is the one action that is about the whole drawer, and every panel
   * has a different one.
   */
  actions?: ReactNode;
  /**
   * A panel that opens on top of this one — an editor for the thing the drawer
   * lists.
   *
   * A slot rather than something the caller renders beside `Drawer`, because
   * where it goes in the DOM is load-bearing: `Modal` decides whether to take
   * Escape by asking whether a `.scrim` exists *inside* its own, so an editor
   * rendered outside this scrim would let Escape close the drawer out from
   * under the editor it opened. Inside the scrim and outside the panel is the
   * one position that works, and it is not a position anybody would guess.
   */
  overlay?: ReactNode;
  children: ReactNode;
}) {
  return (
    <Modal onClose={onClose}>
      <aside
        className={panel ? `drawer ${panel}` : "drawer"}
        // See above. This is the line the extraction exists for.
        onClick={(e) => e.stopPropagation()}
      >
        <DrawerHead title={title} onClose={onClose}>
          {actions}
        </DrawerHead>
        {children}
      </aside>
      {overlay}
    </Modal>
  );
}

/**
 * The title bar on its own.
 *
 * For the panels that cannot use `Drawer` but have the same head: the docked
 * changes pane, the delegate transcript, the MCP drawer, and the two editors
 * that layer over an already-open drawer as a `.modal` rather than a `.drawer`.
 * Those last two borrow the header the way they already borrowed its class,
 * which is why this is named after the class rather than after the drawer.
 *
 * The ✕ rather than the word: seven of the ten panels already spelled it this
 * way, it stays out of the way of the Rescan or Reconnect beside it, and the
 * `aria-label` is what a screen reader reads either way — so the word was
 * carrying nothing the label was not.
 */
export function DrawerHead({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  /** The actions, when there are any. */
  children?: ReactNode;
}) {
  return (
    <header className="drawer-head">
      <h2>{title}</h2>
      {children}
      <button className="drawer-close" onClick={onClose} aria-label="Close">
        ✕
      </button>
    </header>
  );
}
