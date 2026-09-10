/**
 * Types for the Vite plugin in `excalidraw-assets.mjs`.
 *
 * The plugin is plain JavaScript because both Vite configs import it and one of
 * them is loaded from a directory `tsc` does not compile; a declaration beside
 * it is how `vite.config.ts` gets to type-check its own import.
 */
import type { Plugin } from "vite";

/** Where the fonts are served from, relative to the app's origin. */
export declare const ASSET_BASE: string;

/** Serves Excalidraw's fonts from the app's origin, and copies them into a build. */
export declare function excalidrawAssets(): Plugin;
